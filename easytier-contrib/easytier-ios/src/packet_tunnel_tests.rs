use super::*;
use easytier::proto::{
    api::manage::{ListNetworkInstanceRequest, WebClientService, WebClientServiceClientFactory},
    rpc_types::controller::BaseController,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn management_requires_session_key_and_serves_existing_rpc() {
    let directory = std::env::temp_dir().join(format!("easytier-ne-auth-{}", uuid::Uuid::new_v4()));
    let core = PacketTunnelRuntime::new();
    let endpoint = core.start(&directory).unwrap();
    core.runtime.block_on(async {
        let address = (
            std::net::Ipv4Addr::LOCALHOST,
            endpoint["port"].as_u64().unwrap() as u16,
        );
        let mut rejected = tokio::net::TcpStream::connect(address).await.unwrap();
        rejected.write_all(&[0; 32]).await.unwrap();
        let mut byte = [0u8];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), rejected.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let token = endpoint["token"].as_str().unwrap();
        let key = (0..32)
            .map(|i| u8::from_str_radix(&token[i * 2..i * 2 + 2], 16).unwrap())
            .collect::<Vec<_>>();
        stream.write_all(&key).await.unwrap();
        let (read, write) = stream.into_split();
        let tunnel = TunnelWrapper::new(
            FramedReader::new(read, TCP_MTU_BYTES),
            FramedWriter::new(write),
            None,
        );
        let rpc = BidirectRpcManager::new();
        rpc.run_with_tunnel(Box::new(tunnel));
        let client = rpc
            .rpc_client()
            .scoped_client::<WebClientServiceClientFactory<BaseController>>(1, 1, String::new());
        let response = tokio::time::timeout(
            Duration::from_secs(3),
            client.list_network_instance(BaseController::default(), ListNetworkInstanceRequest {}),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(response.inst_ids.is_empty());
    });
    core.stop().unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn network_configuration_survives_extension_restart() {
    let directory =
        std::env::temp_dir().join(format!("easytier-ne-persist-{}", uuid::Uuid::new_v4()));
    let core = PacketTunnelRuntime::new();
    core.start(&directory).unwrap();
    let config = TomlConfigLoader::new_from_str("instance_name = \"restart-test\"\nipv4 = \"10.126.125.1/24\"\nlisteners = []\nflags.no_tun = true\n").unwrap();
    let id = core
        .runtime
        .block_on(async {
            core.manager
                .run_network_instance(config, ConfigFileControl::STATIC_CONFIG)
        })
        .unwrap();
    core.persist().unwrap();
    core.stop().unwrap();
    let restored = PacketTunnelRuntime::new();
    restored.start(&directory).unwrap();
    assert_eq!(restored.manager.instance_ids(), vec![id]);
    assert!(restored.manager.config(id).unwrap().get_flags().no_tun);
    restored.stop().unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn corrupt_saved_configuration_fails_without_replacing_it() {
    let directory =
        std::env::temp_dir().join(format!("easytier-ne-invalid-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let saved = directory.join("networks.json");
    std::fs::write(&saved, "invalid saved data").unwrap();
    let core = PacketTunnelRuntime::new();
    assert!(core.start(&directory).is_err());
    core.stop().unwrap();
    assert_eq!(
        std::fs::read_to_string(saved).unwrap(),
        "invalid saved data"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn second_tun_network_is_rejected_before_runtime_start() {
    let core = PacketTunnelRuntime::new();
    let first = TomlConfigLoader::new_from_str(
        "instance_name = \"first\"\nipv4 = \"10.126.125.1/24\"\nlisteners = []\n",
    )
    .unwrap();
    core.manager.create(first, ()).unwrap();
    let second = TomlConfigLoader::new_from_str(
        "instance_name = \"second\"\nipv4 = \"10.126.125.2/24\"\nlisteners = []\n",
    )
    .unwrap();
    let hooks = crate::packet_tunnel_hooks::PacketTunnelHooks(core.manager.clone());
    core.runtime.block_on(async {
        assert!(
            easytier_core::management::InstanceMutationHooks::pre_run_network_instance(
                &hooks, &second
            )
            .await
            .is_err()
        );
        let mut flags = second.get_flags();
        flags.no_tun = true;
        second.set_flags(flags);
        assert!(
            easytier_core::management::InstanceMutationHooks::pre_run_network_instance(
                &hooks, &second
            )
            .await
            .is_ok()
        );
    });
    core.stop().unwrap();
}
