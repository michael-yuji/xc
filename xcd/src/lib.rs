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
mod auth;
mod config_manager;
mod context;
mod devfs_store;
mod image;
mod network_manager;
mod port;
mod registry;
mod site;
mod task;

pub mod ipc;
use config_manager::ConfigManager;
use context::ServerContext;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

pub async fn xmain() -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt::init();
    let config_path = "/usr/local/etc/xc.conf";
    info!("loading configuration from {config_path}");
    match ConfigManager::load_from_path(config_path) {
        Err(error) => { error!("{error:#?}"); },
        Ok(config_manager) => {
            let xc_config = config_manager.config();
            let path = xc_config.socket_path.to_string();
            info!("config: {xc_config:#?}");
            let context = Arc::new(RwLock::new(ServerContext::new(config_manager)));
            let join_handle = ServerContext::create_channel(context, &path)?;
            join_handle.await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {}
