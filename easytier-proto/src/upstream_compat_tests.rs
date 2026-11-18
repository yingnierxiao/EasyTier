use crate::{api::manage::NetworkConfig, common::Flags};
use prost::Message;

#[test]
fn upstream_fec_wire_tags_keep_their_meaning_with_underlay_binding() {
    // 构造上游发送的字段，验证本地源地址字段不会吞掉 FEC 配置。
    let mut wire = Vec::new();
    for tag in [45, 46, 47] {
        prost::encoding::bool::encode(tag, &true, &mut wire);
    }
    prost::encoding::string::encode(50045, &"192.0.2.10".to_owned(), &mut wire);
    let flags = Flags::decode(wire.as_slice()).unwrap();
    assert!(flags.enable_udp_fec);
    assert!(flags.enable_udp_fec_kcp);
    assert!(flags.enable_udp_fec_quic);
    assert_eq!(flags.bind_address, "192.0.2.10");
    assert_eq!(
        Flags::decode(flags.encode_to_vec().as_slice()).unwrap(),
        flags
    );
}

#[test]
#[allow(deprecated)]
fn upstream_dns_wire_tag_is_distinct_from_underlay_source_address() {
    let mut wire = Vec::new();
    prost::encoding::string::encode(73, &"legacy.example".to_owned(), &mut wire);
    prost::encoding::string::encode(50073, &"2001:db8::10".to_owned(), &mut wire);
    for tag in [78, 79, 80] {
        prost::encoding::bool::encode(tag, &true, &mut wire);
    }
    let config = NetworkConfig::decode(wire.as_slice()).unwrap();
    assert_eq!(config.tld_dns_zone.as_deref(), Some("legacy.example"));
    assert_eq!(config.bind_address.as_deref(), Some("2001:db8::10"));
    assert_eq!(config.enable_udp_fec, Some(true));
    assert_eq!(config.enable_udp_fec_kcp, Some(true));
    assert_eq!(config.enable_udp_fec_quic, Some(true));
    assert_eq!(
        NetworkConfig::decode(config.encode_to_vec().as_slice()).unwrap(),
        config
    );
}

#[test]
fn underlay_binding_keeps_its_json_configuration_name() {
    let flags: Flags = serde_json::from_value(
        serde_json::json!({"bind_address":"192.0.2.10", "enable_udp_fec":true}),
    )
    .unwrap();
    assert_eq!(flags.bind_address, "192.0.2.10");
    assert!(flags.enable_udp_fec);
    let config: NetworkConfig = serde_json::from_value(
        serde_json::json!({"bind_address":"192.0.2.10", "enable_udp_fec":false}),
    )
    .unwrap();
    assert_eq!(config.bind_address.as_deref(), Some("192.0.2.10"));
    assert_eq!(config.enable_udp_fec, Some(false));
}
