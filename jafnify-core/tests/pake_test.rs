use std::net::Ipv4Addr;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};

use jafnify_core::discovery::DiscoveryConfig;
use jafnify_core::pairing::PairingCode;
use jafnify_core::pake::{PakeConfirm, PakeHello, PakeLink, PakePoll};
use jafnify_core::state::Role;
use jafnify_core::KeyPair;

const TEST_PORT: u16 = 49804;
static MULTICAST: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    MULTICAST.lock().unwrap_or_else(|e| e.into_inner())
}

fn cfg() -> DiscoveryConfig {
    DiscoveryConfig {
        iface: Some(Ipv4Addr::LOCALHOST),
        addr: format!("239.255.42.99:{TEST_PORT}").parse().unwrap(),
        ..Default::default()
    }
}

fn link(role: Role, code: &str, own: &KeyPair, port: u16) -> PakeLink {
    PakeLink::open(&cfg(), role, &PairingCode::parse(code).unwrap(), own, port).unwrap()
}

fn drive(joiner: &mut PakeLink, sharer: &mut PakeLink) -> (PakePoll, PakePoll) {
    let start = Instant::now();
    let mut joiner_poll = PakePoll::default();
    let mut sharer_poll = PakePoll::default();
    for step in 0..12 {
        let now = start + Duration::from_millis(50 * step);
        joiner_poll = joiner.poll(now).unwrap();
        sharer_poll = sharer.poll(now).unwrap();
        if joiner_poll.peer.is_some() && sharer_poll.peer.is_some() {
            break;
        }
    }
    (joiner_poll, sharer_poll)
}

#[test]
fn the_same_code_completes_and_both_keys_are_the_peers() {
    let _guard = lock();
    let joiner_key = KeyPair::generate();
    let sharer_key = KeyPair::generate();
    let mut joiner = link(Role::Joiner, "1234", &joiner_key, 0);
    let mut sharer = link(Role::Sharer, "1234", &sharer_key, 9797);

    let (jp, sp) = drive(&mut joiner, &mut sharer);
    let jpeer = jp.peer.expect("joiner accepted no peer");
    let speer = sp.peer.expect("sharer accepted no peer");

    assert_eq!(jpeer.public_key, sharer_key.public_key_bytes());
    assert_eq!(speer.public_key, joiner_key.public_key_bytes());
    assert_eq!(jpeer.endpoint.port(), 9797);
    assert_eq!(speer.endpoint.port(), 0);
    assert!(!jp.old_peer && !sp.old_peer);
}

#[test]
fn a_wrong_code_never_accepts_either_side() {
    let _guard = lock();
    let joiner_key = KeyPair::generate();
    let sharer_key = KeyPair::generate();
    let mut joiner = link(Role::Joiner, "1234", &joiner_key, 0);
    let mut sharer = link(Role::Sharer, "9876", &sharer_key, 9797);

    let (jp, sp) = drive(&mut joiner, &mut sharer);
    assert!(jp.peer.is_none(), "wrong code accepted on the joiner");
    assert!(sp.peer.is_none(), "wrong code accepted on the sharer");
    assert!(jp.wrong_code || sp.wrong_code, "no wrong-code evidence");
}

#[test]
fn hello_and_confirm_frame_layouts_round_trip() {
    let hello = PakeHello {
        role: Role::Joiner,
        nonce: [1; 16],
        share: [2; 33],
    };
    let wire = hello.encode();
    assert_eq!(wire.len(), 52);
    assert_eq!(wire[0], 0x05);
    assert_eq!(wire[1], 2);
    assert_eq!(PakeHello::decode(&wire).unwrap(), hello);
    assert!(PakeHello::decode(&wire[..51]).is_err());

    let key = [9u8; 32];
    let confirm = PakeConfirm::new(Role::Sharer, [3; 16], [4; 16], [5; 32], 9797, &key);
    let wire = confirm.encode();
    assert_eq!(wire.len(), 101);
    assert_eq!(wire[0], 0x06);
    assert_eq!(wire[1], 2);
    assert_eq!(PakeConfirm::decode(&wire).unwrap(), confirm);
    assert!(PakeConfirm::decode(&wire[..100]).is_err());
}

#[test]
fn the_confirm_mac_binds_key_port_and_nonces() {
    let key = [9u8; 32];
    let confirm = PakeConfirm::new(Role::Sharer, [3; 16], [4; 16], [5; 32], 9797, &key);
    assert!(confirm.verify_with_key(&key));
    assert!(!confirm.verify_with_key(&[8u8; 32]));

    let mut altered = confirm.clone();
    altered.port = 9798;
    assert!(!altered.verify_with_key(&key));

    let mut altered = confirm.clone();
    altered.public_key = [6; 32];
    assert!(!altered.verify_with_key(&key));

    let mut altered = confirm.clone();
    altered.n_echo = [7; 16];
    assert!(!altered.verify_with_key(&key));
}

fn captured_pair() -> (PakeHello, PakeHello, PakeConfirm) {
    let joiner_key = KeyPair::generate();
    let sharer_key = KeyPair::generate();
    let mut joiner = link(Role::Joiner, "1234", &joiner_key, 0);
    let mut sharer = link(Role::Sharer, "1234", &sharer_key, 9797);
    let (jp, sp) = drive(&mut joiner, &mut sharer);
    assert!(jp.peer.is_some() && sp.peer.is_some());
    let joiner_hello = joiner.hello();
    let sharer_hello = sharer.hello();
    let confirm = sharer.own_confirm().expect("sharer sent no confirm");
    (joiner_hello, sharer_hello, confirm)
}

fn code_keyed_mac(code: &str, confirm: &PakeConfirm) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(code.as_bytes()).unwrap();
    mac.update(b"jafnify/v2/confirm");
    mac.update(&[2u8]);
    mac.update(&[match confirm.role {
        Role::Sharer => 0,
        Role::Joiner => 1,
    }]);
    mac.update(&confirm.n_self);
    mac.update(&confirm.n_echo);
    mac.update(&confirm.public_key);
    mac.update(&confirm.port.to_be_bytes());
    mac.finalize().into_bytes().into()
}

fn assert_no_offline_guess(
    joiner_hello: &PakeHello,
    sharer_hello: &PakeHello,
    confirm: &PakeConfirm,
    step: usize,
) {
    let id_a = Identity::new(b"jafnify/v2/joiner");
    let id_b = Identity::new(b"jafnify/v2/sharer");
    for n in (1..=9999usize).step_by(step) {
        let code = format!("{n:04}");
        if code.starts_with('0') {
            continue;
        }
        let (session, _share) =
            Spake2::<Ed25519Group>::start_a(&Password::new(code.as_bytes()), &id_a, &id_b);
        let key: [u8; 32] = session
            .finish(&sharer_hello.share)
            .unwrap()
            .try_into()
            .unwrap();
        assert!(
            !confirm.verify_with_key(&key),
            "a guessed code verified the captured confirm"
        );
        assert_ne!(
            code_keyed_mac(&code, confirm),
            confirm.mac,
            "the confirm MAC is still keyed by the code"
        );
        assert!(!joiner_hello.share.iter().all(|b| *b == 0));
    }
}

#[test]
fn a_captured_transcript_does_not_verify_a_guessed_code() {
    let _guard = lock();
    let (joiner_hello, sharer_hello, confirm) = captured_pair();
    assert_no_offline_guess(&joiner_hello, &sharer_hello, &confirm, 97);
}

#[test]
#[ignore = "full 9,000-code sweep; run with --ignored"]
fn every_code_of_the_full_space_fails_offline_verification() {
    let _guard = lock();
    let (joiner_hello, sharer_hello, confirm) = captured_pair();
    assert_no_offline_guess(&joiner_hello, &sharer_hello, &confirm, 1);
}
