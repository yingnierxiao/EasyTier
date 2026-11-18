use easytier::common::config::{ConfigLoader, TomlConfigLoader};
use easytier::instance::factory::NativeInstanceManager;
use easytier_core::instance::CoreInstanceState;
use std::sync::Arc;

pub struct PacketTunnelHooks(pub Arc<NativeInstanceManager>);

#[async_trait::async_trait]
impl easytier_core::management::InstanceMutationHooks for PacketTunnelHooks {
    async fn pre_run_network_instance(&self, config: &TomlConfigLoader) -> Result<(), String> {
        if config.get_flags().no_tun {
            return Ok(());
        }
        for id in self.0.instance_ids() {
            if id == config.get_id() {
                continue;
            }
            let tun_enabled = self.0.config(id).is_some_and(|c| !c.get_flags().no_tun);
            let active = self
                .0
                .instance(id)
                .is_some_and(|i| i.state() != CoreInstanceState::Stopped);
            if tun_enabled && active {
                return Err("iOS 仅支持一个启用虚拟网卡的网络，请先停止当前网络".into());
            }
        }
        Ok(())
    }
}
