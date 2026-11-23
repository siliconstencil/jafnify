//! The root helper keeps the two route verbs for installs that predate the
//! small route binary. Both entry points share the same mark-scoped command
//! lists; route_test.rs pins those lists and the guard.

use jafnify_linux::helper::{parse_command, HelperCommand};
use jafnify_linux::route::{self, RouteCommand};

#[test]
fn the_helper_accepts_exactly_the_two_route_verbs() {
    let one = |verb: &str| vec![verb.to_string()];
    assert_eq!(
        parse_command(&one("route-fallback")),
        Ok(HelperCommand::RouteFallback)
    );
    assert_eq!(
        parse_command(&one("route-restore")),
        Ok(HelperCommand::RouteRestore)
    );
    assert!(parse_command(&["route-fallback".to_string(), "x".to_string()]).is_err());
    assert!(parse_command(&one("route-main")).is_err());
}

#[test]
fn the_route_binary_accepts_the_same_two_verbs() {
    let one = |verb: &str| vec![verb.to_string()];
    assert_eq!(
        route::parse(&one("route-fallback")),
        Ok(RouteCommand::Fallback)
    );
    assert_eq!(
        route::parse(&one("route-restore")),
        Ok(RouteCommand::Restore)
    );
    assert!(route::parse(&one("up")).is_err());
    assert!(route::parse(&[]).is_err());
}
