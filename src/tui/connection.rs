use radio_tui::shared::protocol::{Command, Message};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct DaemonConnection {
    stream: TcpStream,
    read_buffer: Vec<u8>,
}

impl DaemonConnection {
    pub async fn connect(address: &str) -> anyhow::Result<Self> {
        let stream = TcpStream::connect(address).await?;
        Ok(Self {
            stream,
            read_buffer: Vec::with_capacity(4096),
        })
    }
    
    pub async fn send_command(&mut self,
        cmd: Command,
    ) -> anyhow::Result<()> {
        let msg = Message::Command(cmd);
        let encoded = msg.encode()?;
        self.stream.write_all(&encoded).await?;
        Ok(())
    }
    
    pub async fn receive_message(&mut self) -> anyhow::Result<Option<Message>> {
        let mut buf = vec![0u8; 4096];
        
        match self.stream.read(&mut buf).await {
            Ok(0) => Ok(None), // Connection closed
            Ok(n) => {
                self.read_buffer.extend_from_slice(&buf[..n]);
                
                // Try to decode
                if self.read_buffer.len() >= 4 {
                    match Message::decode(&self.read_buffer) {
                        Ok((msg, consumed)) => {
                            self.read_buffer.drain(..consumed);
                            Ok(Some(msg))
                        }
                        Err(_) => {
                            // Not enough data
                            Ok(None)
                        }
                    }
                } else {
                    Ok(None)
                }
            }
            Err(e) => Err(anyhow::anyhow!("Read error: {}", e)),
        }
    }
    
    pub async fn process_incoming(&mut self,
    ) -> anyhow::Result<()> {
        // Non-blocking check for messages
        self.stream.readable().await?;
        Ok(())
    }
}
