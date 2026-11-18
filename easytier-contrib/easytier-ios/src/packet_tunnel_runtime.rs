use anyhow::Context;
use easytier::{
    common::config::{ConfigLoader, TomlConfigLoader},
    instance::factory::{NativeInstanceManager, native_instance_manager_with_runtime},
};
use easytier_core::{
    management::{ConfigFileControl, UnsupportedConfigFileStorage, register_management_rpc},
    rpc::bidirect::BidirectRpcManager,
    tunnel::{
        framed::{FramedReader, FramedWriter, TCP_MTU_BYTES},
        wrapper::TunnelWrapper,
    },
};
use rand::RngCore;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    io::AsyncReadExt, net::TcpListener, runtime::Runtime, sync::Semaphore, task::JoinHandle,
};

/// 管理端口只绑定回环；会话密钥仅通过系统的宿主与扩展消息通道交付。
pub struct PacketTunnelRuntime {
    runtime: Runtime,
    manager: Arc<NativeInstanceManager>,
    server: Mutex<Option<JoinHandle<()>>>,
    directory: Mutex<Option<PathBuf>>,
    saved: Mutex<String>,
    #[cfg(target_os = "ios")]
    attached: Mutex<
        Option<(
            std::sync::Weak<easytier::instance::factory::NativeCoreInstance>,
            Vec<String>,
            bool,
            u32,
        )>,
    >,
    web: Mutex<Option<easytier::web_client::WebClient>>,
}

impl PacketTunnelRuntime {
    fn new() -> Self {
        easytier::utils::init_crypto_provider();
        // 扩展的内存和线程预算有限，不按设备 CPU 数量创建工作线程。
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("packet tunnel runtime");
        let manager = Arc::new(native_instance_manager_with_runtime(
            runtime.handle().clone(),
        ));
        Self {
            runtime,
            manager,
            server: Mutex::new(None),
            directory: Mutex::new(None),
            saved: Mutex::new(String::new()),
            #[cfg(target_os = "ios")]
            attached: Mutex::new(None),
            web: Mutex::new(None),
        }
    }

    pub fn start(&self, directory: &Path) -> anyhow::Result<serde_json::Value> {
        anyhow::ensure!(
            self.server.lock().unwrap().is_none(),
            "packet tunnel runtime already started"
        );
        let result = self.start_inner(directory);
        if result.is_err() {
            if let Some(task) = self.server.lock().unwrap().take() {
                task.abort();
                let _ = self.runtime.block_on(task);
            }
            *self.web.lock().unwrap() = None;
            let _ = self
                .runtime
                .block_on(self.manager.retain_network_instances(&[]));
            *self.directory.lock().unwrap() = None;
            self.saved.lock().unwrap().clear();
        }
        result
    }

    fn start_inner(&self, directory: &Path) -> anyhow::Result<serde_json::Value> {
        std::fs::create_dir_all(directory)?;
        *self.directory.lock().unwrap() = Some(directory.to_owned());
        let stored = match std::fs::read_to_string(directory.join("networks.json")) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error).context("cannot read saved networks"),
        };
        if let Some(text) = stored {
            let configs: Vec<String> =
                serde_json::from_str(&text).context("invalid saved networks")?;
            self.runtime.block_on(async {
                for config in configs {
                    let config = TomlConfigLoader::new_from_str(&config)?;
                    easytier_core::management::InstanceMutationHooks::pre_run_network_instance(
                        &crate::packet_tunnel_hooks::PacketTunnelHooks(self.manager.clone()),
                        &config,
                    )
                    .await
                    .map_err(anyhow::Error::msg)?;
                    self.manager
                        .run_network_instance(config, ConfigFileControl::STATIC_CONFIG)?;
                }
                Ok::<_, anyhow::Error>(())
            })?;
            *self.saved.lock().unwrap() = text;
        }
        let listener = self.runtime.block_on(TcpListener::bind("127.0.0.1:0"))?;
        let port = listener.local_addr()?.port();
        let mut key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        let token = key.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let manager = self.manager.clone();
        let task = self.runtime.spawn(async move {
            let limit = Arc::new(Semaphore::new(8));
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((mut stream, _)) = accepted else { break };
                        let Ok(permit) = limit.clone().try_acquire_owned() else { continue };
                        let manager = manager.clone();
                        tasks.spawn(async move {
                            let _permit = permit;
                            let mut supplied = [0u8; 32];
                            if !matches!(tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut supplied)).await, Ok(Ok(_))) { return; }
                            // 固定长度比较，不向未认证客户端注册管理服务。
                            if supplied.iter().zip(key).fold(0u8, |difference, (a, b)| difference | (*a ^ b)) != 0 { return; }
                            let (read, write) = stream.into_split();
                            let tunnel = TunnelWrapper::new(FramedReader::new(read, TCP_MTU_BYTES), FramedWriter::new(write), None);
                            let rpc = BidirectRpcManager::new().set_rx_timeout(None);
                            register_management_rpc(manager.clone(), rpc.rpc_server().registry(), Arc::new(crate::packet_tunnel_hooks::PacketTunnelHooks(manager.clone())), Arc::new(UnsupportedConfigFileStorage), Arc::new(easytier::rpc_service::logger::NativeLoggerControl));
                            rpc.run_with_tunnel(Box::new(tunnel));
                            rpc.wait().await;
                        });
                    }
                    _ = tasks.join_next(), if !tasks.is_empty() => {}
                }
            }
        });
        *self.server.lock().unwrap() = Some(task);
        match std::fs::read_to_string(directory.join("web-url")) {
            Ok(url) if !url.is_empty() => {
                self.configure_web(Some(url))?;
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
        Ok(serde_json::json!({"port":port, "token":token}))
    }

    fn persist(&self) -> anyhow::Result<()> {
        let mut configs = self
            .manager
            .instance_ids()
            .into_iter()
            .filter_map(|id| self.manager.config(id))
            .map(|c| c.dump())
            .collect::<Vec<_>>();
        configs.sort();
        let text = serde_json::to_string(&configs)?;
        let mut saved = self.saved.lock().unwrap();
        if *saved == text {
            return Ok(());
        }
        let directory = self.directory.lock().unwrap();
        let directory = directory
            .as_ref()
            .context("runtime directory unavailable")?;
        let temporary = directory.join("networks.next");
        Self::write_private(&temporary, text.as_bytes())?;
        std::fs::rename(temporary, directory.join("networks.json"))?;
        *saved = text;
        Ok(())
    }

    #[cfg(target_os = "ios")]
    pub fn snapshot(&self, fd: i32) -> anyhow::Result<serde_json::Value> {
        let settings =
            self.runtime
                .block_on(easytier::instance::factory::packet_tunnel::snapshot(
                    &self.manager,
                    fd,
                    &mut self.attached.lock().unwrap(),
                ))?;
        self.persist()?;
        Ok(serde_json::to_value(settings)?)
    }

    fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }

    pub fn configure_web(&self, url: Option<String>) -> anyhow::Result<serde_json::Value> {
        let configured_url = url.clone().unwrap_or_default();
        if let Some(url) = url.filter(|u| !u.is_empty()) {
            let directory = self
                .directory
                .lock()
                .unwrap()
                .clone()
                .context("runtime directory unavailable")?;
            let client = self.runtime.block_on(easytier::web_client::run_web_client(
                &url,
                easytier::common::MachineIdOptions {
                    explicit_machine_id: None,
                    state_dir: Some(directory),
                },
                None,
                false,
                self.manager.clone(),
                Some(Arc::new(crate::packet_tunnel_hooks::PacketTunnelHooks(
                    self.manager.clone(),
                ))),
            ))?;
            *self.web.lock().unwrap() = Some(client);
        } else {
            *self.web.lock().unwrap() = None;
        }
        let directory = self.directory.lock().unwrap();
        let directory = directory
            .as_ref()
            .context("runtime directory unavailable")?;
        Self::write_private(&directory.join("web-url.next"), configured_url.as_bytes())?;
        std::fs::rename(directory.join("web-url.next"), directory.join("web-url"))?;
        Ok(
            serde_json::json!({"connected":self.web.lock().unwrap().as_ref().is_some_and(|w| w.is_connected())}),
        )
    }

    pub fn web_status(&self) -> serde_json::Value {
        serde_json::json!({"connected":self.web.lock().unwrap().as_ref().is_some_and(|w| w.is_connected())})
    }

    pub fn stop(&self) -> anyhow::Result<()> {
        *self.web.lock().unwrap() = None;
        if let Some(task) = self.server.lock().unwrap().take() {
            task.abort();
            let _ = self.runtime.block_on(task);
        }
        let configured = self.directory.lock().unwrap().is_some();
        let persisted = if configured { self.persist() } else { Ok(()) };
        let stopped = self
            .runtime
            .block_on(self.manager.retain_network_instances(&[]));
        #[cfg(target_os = "ios")]
        {
            *self.attached.lock().unwrap() = None;
        }
        *self.directory.lock().unwrap() = None;
        self.saved.lock().unwrap().clear();
        stopped?;
        persisted
    }
}

pub fn runtime() -> &'static PacketTunnelRuntime {
    static INSTANCE: OnceLock<PacketTunnelRuntime> = OnceLock::new();
    INSTANCE.get_or_init(PacketTunnelRuntime::new)
}

#[cfg(test)]
#[path = "packet_tunnel_tests.rs"]
mod tests;
