use std::io;
use std::io::Error;
use std::sync::atomic::Ordering::Relaxed;
use crate::{message, strings, size, global_variable};
use crate::channel_exec::ChannelExec;
use crate::channel_shell::ChannelShell;
use crate::encryption::ChaCha20Poly1305;
use crate::error::{SshError, SshErrorKind};
use crate::hash::HASH;
use crate::key_agreement::KeyAgreement;
use crate::packet::{Data, Packet};
use crate::tcp::Client;

pub struct Channel {
    pub(crate) stream: Client,
    pub(crate) server_channel: u32,
    pub(crate) client_channel: u32,
    pub(crate) key_agreement: KeyAgreement
}



impl Channel {

    pub fn other_info(&mut self, message_code: u8, result: Vec<u8>) -> Result<(), SshError> {
        match message_code {
            message::SSH_MSG_GLOBAL_REQUEST => {
                let mut data = Data::new();
                data.put_u8(message::SSH_MSG_REQUEST_FAILURE);
                let mut packet = Packet::from(data);
                packet.build();
                self.stream.write(packet.as_slice())?;
            }
            message::SSH_MSG_KEXINIT => {
                let data = Packet::processing_data(result);
                // 重置加密算法
                if global_variable::IS_ENCRYPT.load(Relaxed) {
                    global_variable::IS_ENCRYPT.store(false, Relaxed);
                    global_variable::update_encryption_key(None);
                }
                // 密钥协商
                self.key_agreement.algorithm_negotiation(data, &mut self.stream)?;
                // 发送公钥
                self.key_agreement.send_public_key(&mut self.stream)?;
            }
            message::SSH_MSG_KEX_ECDH_REPLY => {
                // 生成session_id并且获取signature
                let sig = self.key_agreement.generate_session_id_and_get_signature(result)?;
                // 验签
                self.key_agreement.verify_signature(&sig)?;
                // 新的密钥
                self.key_agreement.new_keys(&mut self.stream)?;

                // 修改加密算法
                let hash =
                    HASH::new(&self.key_agreement.h.k,
                              &self.key_agreement.session_id, &self.key_agreement.session_id);
                let poly1305 = ChaCha20Poly1305::new(hash);
                global_variable::IS_ENCRYPT.store(true, Relaxed);
                global_variable::update_encryption_key(Some(poly1305));
            }
            // 通道大小 暂不处理
            message::SSH_MSG_CHANNEL_WINDOW_ADJUST => {
            }
            message::SSH_MSG_CHANNEL_REQUEST => {
            }
            message::SSH_MSG_CHANNEL_SUCCESS => {
            }
            message::SSH_MSG_CHANNEL_FAILURE => {
                return Err(SshError::from(SshErrorKind::ChannelFailureError))
            }
            message::SSH_MSG_CHANNEL_CLOSE => {
                let mut data = Data::new();
                data.put_u8(message::SSH_MSG_CHANNEL_CLOSE)
                    .put_u32(self.server_channel);
                let mut packet = Packet::from(data);
                packet.build();
                self.stream.write(packet.as_slice())?;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn window_adjust(&mut self) -> Result<(), SshError> {
        if self.stream.sender_window_size >= (size::LOCAL_WINDOW_SIZE / 2) {
            let mut data = Data::new();
            data.put_u8(message::SSH_MSG_CHANNEL_WINDOW_ADJUST)
                .put_u32(self.server_channel)
                .put_u32(size::LOCAL_WINDOW_SIZE - self.stream.sender_window_size);
            let mut packet = Packet::from(data);
            packet.build();
            self.stream.write(packet.as_slice())?;
            self.stream.sender_window_size = 0;
        }
        Ok(())
    }

    pub fn open_shell(mut self) -> Result<ChannelShell, SshError> {
        loop {
            let results = self.stream.read()?;
            for buf in results {
                let message_code = buf[5];
                match message_code {
                    message::SSH_MSG_CHANNEL_OPEN_CONFIRMATION => {
                        let mut data = Packet::processing_data(buf);
                        data.get_u8();
                        data.get_u32();
                        self.server_channel = data.get_u32();
                        // 请求伪终端
                        self.request_pty()?;
                        // 打开shell通道
                        self.get_shell()?;
                    }
                    message::SSH_MSG_CHANNEL_SUCCESS => {
                        return Ok(ChannelShell(self))
                    }
                    _ => self.other_info(message_code, buf)?
                }
            }
        }
    }

    pub fn open_exec(mut self) -> Result<ChannelExec, SshError> {
        loop {
            let results = self.stream.read()?;
            for buf in results {
                let message_code = buf[5];
                match message_code {
                    message::SSH_MSG_CHANNEL_OPEN_CONFIRMATION => {
                        let mut data = Packet::processing_data(buf);
                        data.get_u8();
                        data.get_u32();
                        self.server_channel = data.get_u32();
                        // 请求伪终端
                        self.request_pty()?;
                        return Ok(ChannelExec(self))
                    }
                    _ => self.other_info(message_code, buf)?
                }
            }
        }
    }

    pub fn close(mut self) -> Result<(), SshError> {
        let mut data = Data::new();
        data.put_u8(message::SSH_MSG_CHANNEL_CLOSE)
            .put_u32(self.server_channel);
        let mut packet = Packet::from(data);
        packet.build();
        self.stream.write(packet.as_slice())?;
        let date_time = chrono::Local::now();
        let timeout = date_time.timestamp_millis() + 1500;
        loop {
            if date_time.timestamp_millis() >= timeout {
                return Err(SshError::from(Error::from(io::ErrorKind::TimedOut)))
            }
            let results = self.stream.read()?;
            for buf in results {
                let mut data = Packet::processing_data(buf);
                let message_code = data.get_u8();
                let channel_num = data.get_u32();
                if message_code == message::SSH_MSG_CHANNEL_CLOSE
                    && channel_num == self.client_channel
                {
                    return Ok(())
                }
            }
        }
    }

    fn get_shell(&mut self) -> Result<(), SshError> {
        let mut data = Data::new();
        data.put_u8(message::SSH_MSG_CHANNEL_REQUEST)
            .put_u32(self.server_channel)
            .put_str(strings::SHELL)
            .put_u8(true as u8);
        let mut packet = Packet::from(data);
        packet.build();
        Ok(self.stream.write(packet.as_slice())?)
    }

    fn request_pty(&mut self) -> Result<(), SshError> {
        let mut data = Data::new();
        data.put_u8(message::SSH_MSG_CHANNEL_REQUEST)
            .put_u32(self.server_channel)
            .put_str(strings::PTY_REQ)
            .put_u8(false as u8)
            .put_str(strings::XTERM_VAR)
            .put_u32(80)
            .put_u32(24)
            .put_u32(640)
            .put_u32(480);
        let model = [
            128,                  // TTY_OP_ISPEED
            0, 1, 0xc2, 0,        // 115200
            129,                  // TTY_OP_OSPEED
            0, 1, 0xc2, 0,        // 115200 again
            0_u8,                 // TTY_OP_END
        ];
        data.put_bytes(&model);
        let mut packet = Packet::from(data);
        packet.build();
        Ok(self.stream.write(packet.as_slice())?)
    }
}