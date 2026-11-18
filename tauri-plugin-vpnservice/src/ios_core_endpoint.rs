/// 仅通过宿主与扩展消息传递的管理连接凭据，不实现 Debug 以免意外记录密钥。
#[derive(serde::Deserialize)]
pub struct IosCoreEndpoint {
    pub port: u16,
    pub token: String,
}
