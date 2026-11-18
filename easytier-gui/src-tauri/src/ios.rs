use easytier_core::tunnel::{
    Tunnel,
    framed::{FramedReader, FramedWriter, TCP_MTU_BYTES},
    wrapper::TunnelWrapper,
};
use tauri_plugin_vpnservice::VpnserviceExt;
use tokio::io::AsyncWriteExt;

pub async fn connect(app: tauri::AppHandle) -> anyhow::Result<Box<dyn Tunnel>> {
    let endpoint =
        tokio::task::spawn_blocking(move || app.vpnservice().connect_ios_core()).await??;
    anyhow::ensure!(
        endpoint.token.len() == 64
            && endpoint.token.bytes().all(|b| b.is_ascii_hexdigit())
            && endpoint.port != 0,
        "invalid extension endpoint"
    );
    let key = (0..32)
        .map(|i| u8::from_str_radix(&endpoint.token[i * 2..i * 2 + 2], 16))
        .collect::<Result<Vec<_>, _>>()?;
    let mut stream =
        tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, endpoint.port)).await?;
    stream.write_all(&key).await?;
    let (reader, writer) = stream.into_split();
    Ok(Box::new(TunnelWrapper::new(
        FramedReader::new(reader, TCP_MTU_BYTES),
        FramedWriter::new(writer),
        None,
    )))
}

pub async fn command(
    app: tauri::AppHandle,
    command: &'static str,
    url: Option<String>,
) -> Result<serde_json::Value, String> {
    tokio::task::spawn_blocking(move || app.vpnservice().ios_core_command(command, url))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}
