use serde::de::DeserializeOwned;
use tauri::{
    AppHandle, Runtime,
    plugin::{PluginApi, PluginHandle},
};

use crate::models::*;

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "com.plugin.vpnservice";

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_vpnservice);

// initializes the Kotlin or Swift plugin classes
pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> crate::Result<Vpnservice<R>> {
    #[cfg(target_os = "android")]
    let handle = api.register_android_plugin(PLUGIN_IDENTIFIER, "VpnServicePlugin")?;
    #[cfg(target_os = "ios")]
    let handle = api.register_ios_plugin(init_plugin_vpnservice)?;
    Ok(Vpnservice(handle))
}

/// Access to the vpnservice APIs.
pub struct Vpnservice<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> Vpnservice<R> {
    #[cfg(target_os = "ios")]
    pub fn connect_ios_core(&self) -> crate::Result<crate::IosCoreEndpoint> {
        self.0
            .run_mobile_plugin("connectCore", VoidRequest {})
            .map_err(Into::into)
    }

    #[cfg(target_os = "ios")]
    pub fn ios_core_command(
        &self,
        command: &str,
        url: Option<String>,
    ) -> crate::Result<serde_json::Value> {
        self.0
            .run_mobile_plugin(
                "coreCommand",
                serde_json::json!({"command":command,"url":url}),
            )
            .map_err(Into::into)
    }

    pub fn ping(&self, payload: PingRequest) -> crate::Result<PingResponse> {
        self.0
            .run_mobile_plugin("ping", payload)
            .map_err(Into::into)
    }

    pub fn prepare_vpn(&self, payload: VoidRequest) -> crate::Result<Status> {
        self.0
            .run_mobile_plugin("prepare_vpn", payload)
            .map_err(Into::into)
    }

    pub fn start_vpn(&self, payload: StartVpnRequest) -> crate::Result<Status> {
        self.0
            .run_mobile_plugin("start_vpn", payload)
            .map_err(Into::into)
    }

    pub fn stop_vpn(&self, payload: VoidRequest) -> crate::Result<Status> {
        self.0
            .run_mobile_plugin("stop_vpn", payload)
            .map_err(Into::into)
    }

    pub fn get_vpn_status(&self, payload: VoidRequest) -> crate::Result<VpnStatus> {
        self.0
            .run_mobile_plugin("get_vpn_status", payload)
            .map_err(Into::into)
    }

    pub fn consume_vpn_tile_action(
        &self,
        payload: VoidRequest,
    ) -> crate::Result<VpnTileActionResponse> {
        self.0
            .run_mobile_plugin("consume_vpn_tile_action", payload)
            .map_err(Into::into)
    }
}
