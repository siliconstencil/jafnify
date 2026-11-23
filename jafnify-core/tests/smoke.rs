use jafnify_core::{ALLOWED_IPS, JOINER_IP, MTU, SHARER_IP};

#[test]
fn constants_match_spec() {
    assert_eq!(MTU, 1280);
    assert_eq!(ALLOWED_IPS, "0.0.0.0/0");
    assert_eq!(SHARER_IP, "10.9.9.1");
    assert_eq!(JOINER_IP, "10.9.9.2");
}
