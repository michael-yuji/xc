// Copyright (c) 2023 Yan Ka, Chiu.
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions
// are met:
// 1. Redistributions of source code must retain the above copyright
//    notice, this list of conditions, and the following disclaimer,
//    without modification, immediately at the beginning of the file.
// 2. The name of the author may not be used to endorse or promote products
//    derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
// ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
// ARE DISCLAIMED. IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE FOR
// ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
// DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
// OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
// HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
// LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
// OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
// SUCH DAMAGE.
use crate::packet::codec::json::JsonPacket;
use crate::packet::TypedPacket;
use crate::proto::{IpcError, Request, Response};
use crate::transport::tokio_io::AsyncPacketTransport;
use crate::transport::ChannelError;
use crate::util::ExtractInner;
use async_trait::async_trait;
use freebsd::libc::ENOENT;
use freebsd::net::UnixCredential;
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

#[usdt::provider]
mod ipc_server_provider {
    fn received_request(method: &str, data: &serde_json::Value, fds: &[i32]) {}
    fn send_response(method: &str, errno: i32, data: &serde_json::Value, fds: &[i32]) {}
    fn accepted_stream(fd: i32) {}
}

pub struct ConnectionContext<V: Send + Sync> {
    req_count: usize,
    pub unix_credential: Option<UnixCredential>,
    pub udata: Option<V>,
}

impl<V: Send + Sync> ExtractInner for ConnectionContext<V> {
    type Inner = V;
}

impl<V: Send + Sync> Default for ConnectionContext<V> {
    fn default() -> Self {
        ConnectionContext {
            req_count: 0,
            udata: None,
            unix_credential: None,
        }
    }
}

#[async_trait]
pub trait Method<T: 'static, V: Send + Sync>: Send + Sync {
    fn identifier(&self) -> &'static str;
    async fn apply(
        &self,
        context: Arc<T>,
        conn_ctx: &mut ConnectionContext<V>,
        request: JsonPacket,
    ) -> TypedPacket<Response>;
}

type Methods<T, V> = Arc<RwLock<HashMap<String, Box<dyn Method<T, V>>>>>;

pub enum StreamEvent {
    ConnectionEstablished,
    ConnectionClosed,
}

#[async_trait]
pub trait StreamDelegate<T: 'static, V: Sync + Send>: Send + Sync {
    async fn on_event(
        &self,
        context: Arc<T>,
        conn_ctx: &mut ConnectionContext<V>,
        event: StreamEvent,
    ) -> anyhow::Result<()>;
}

pub struct Stream<T: Send + Sync + 'static, V> {
    stream: UnixStream,
    context: Arc<T>,
    methods: Methods<T, V>,
    delegates: Arc<RwLock<Vec<Box<dyn StreamDelegate<T, V>>>>>,
}

pub struct Service<T: Send + Sync + 'static, V> {
    listener: UnixListener,
    context: Arc<T>,
    methods: Methods<T, V>,
    delegates: Arc<RwLock<Vec<Box<dyn StreamDelegate<T, V>>>>>,
}

impl<T: Send + Sync + 'static, V: Send + Sync> Stream<T, V> {
    async fn inner(&mut self, local: &mut ConnectionContext<V>) -> Result<(), std::io::Error> {
        async fn ipc_recv_request(
            stream: &mut UnixStream,
        ) -> Result<(String, JsonPacket), ChannelError<IpcError>> {
            let packet = stream
                .recv_packet()
                .await
                .map_err(|e| e.map(IpcError::Io))?;
            let json_packet = JsonPacket::new(packet).map_err(IpcError::Serde)?;
            let request: Request =
                serde_json::from_value(json_packet.data).map_err(IpcError::Serde)?;
            let repack = JsonPacket {
                data: request.value,
                fds: json_packet.fds,
            };
            Ok((request.method, repack))
        }

        while let Ok((method, packet)) = ipc_recv_request(&mut self.stream).await {
            ipc_server_provider::received_request!(|| (&method, &packet.data, &packet.fds));

            tracing::debug!(">>>>> method {method}");
            local.req_count += 1;
            let context = self.context.clone();
            let response = {
                let methods = self.methods.read().await;
                match methods.get(&method) {
                    None => {
                        let value = serde_json::json!({
                            "error": format!("ipc method {method} not found")
                        });
                        let response = Response {
                            errno: ENOENT,
                            value,
                        };
                        TypedPacket {
                            data: response,
                            fds: Vec::new(),
                        }
                    }
                    Some(method) => method.apply(context, local, packet).await,
                }
            };

            ipc_server_provider::send_response!(|| (
                &method,
                response.data.errno,
                &response.data.value,
                &response.fds
            ));

            let packet = response.map(|data| serde_json::to_vec(&data).unwrap());
            self.stream.send_packet(&packet).await.unwrap();
            tracing::debug!("<<<<< method {method}");
        }
        Ok(())
    }

    async fn activate(&mut self) -> Result<(), std::io::Error> {
        let mut local: ConnectionContext<V> = ConnectionContext {
            unix_credential: Some(UnixCredential::from_socket(&self.stream)?),
            ..ConnectionContext::default()
        };
        let _result = self.inner(&mut local).await;
        {
            let context = self.context.clone();
            for delegate in { self.delegates.read().await }.iter() {
                _ = delegate
                    .on_event(context.clone(), &mut local, StreamEvent::ConnectionClosed)
                    .await;
            }
        }
        Ok(())
    }
}

impl<T: Send + Sync + 'static, V: 'static + Send + Sync> Service<T, V> {
    pub fn bind(path: impl AsRef<Path>, context: Arc<T>) -> Result<Service<T, V>, std::io::Error> {
        let listener = UnixListener::bind(path)?;
        let service = Service {
            listener,
            context,
            methods: Arc::new(RwLock::new(HashMap::new())),
            delegates: Arc::new(RwLock::new(Vec::new())),
        };
        Ok(service)
    }
    pub async fn register(&mut self, handler: impl Method<T, V> + 'static) {
        let mut map = self.methods.write().await;
        map.insert(handler.identifier().to_string(), Box::new(handler));
    }
    pub async fn register_event_delegate(&mut self, delegate: impl StreamDelegate<T, V> + 'static) {
        let mut delegates = self.delegates.write().await;
        delegates.push(Box::new(delegate));
    }
    pub async fn accept(&mut self) -> Result<JoinHandle<()>, std::io::Error> {
        let (stream, _) = self.listener.accept().await?;
        ipc_server_provider::accepted_stream!(|| (stream.as_raw_fd()));
        let methods = self.methods.clone();
        let context = self.context.clone();
        let delegates = self.delegates.clone();
        let x = tokio::spawn(async move {
            let mut stream = Stream {
                methods,
                stream,
                context,
                delegates,
            };
            stream.activate().await.unwrap()
        });
        Ok(x)
    }
    pub async fn start(&mut self) {
        loop {
            self.accept().await.unwrap();
        }
    }
}
