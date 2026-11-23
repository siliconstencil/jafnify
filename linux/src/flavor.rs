//! Product identity for installed names and paths.
//!
//! The default build is the product, "jafnify". Building with
//! `--features test-flavor` produces a side-by-side test install,
//! "jafnify-test", that shares nothing installed with the product: its own
//! binaries, libexec directory, polkit actions and rule, user service, config
//! and runtime directories. Both use the tunnel interface `jafn0`, so only one
//! of them may be connected at a time.
//!
//! Rule: in the test flavor every installed name that contains `jafnify`
//! contains `jafnify-test` instead. [`flavored`] applies that rule to the
//! fixed install scripts and the embedded policy and desktop entry.

#[cfg(not(feature = "test-flavor"))]
mod ids {
    pub const NAME: &str = "jafnify";
    pub const DISPLAY: &str = "Jafnify";
    pub const HELPER_PATH: &str = "/usr/libexec/jafnify/jafnify-helper";
    pub const ROUTE_PATH: &str = "/usr/libexec/jafnify/jafnify-route";
    pub const ACTION_TUNNEL: &str = "org.jafnify.tunnel";
    pub const ACTION_ROUTE: &str = "org.jafnify.route";
    pub const RULE_FILE: &str = "49-jafnify.rules";
    pub const RULES_PATH: &str = "/etc/polkit-1/rules.d/49-jafnify.rules";
    pub const UNIT: &str = "jafnify.service";
}

#[cfg(feature = "test-flavor")]
mod ids {
    pub const NAME: &str = "jafnify-test";
    pub const DISPLAY: &str = "Jafnify Test";
    pub const HELPER_PATH: &str = "/usr/libexec/jafnify-test/jafnify-test-helper";
    pub const ROUTE_PATH: &str = "/usr/libexec/jafnify-test/jafnify-test-route";
    pub const ACTION_TUNNEL: &str = "org.jafnify-test.tunnel";
    pub const ACTION_ROUTE: &str = "org.jafnify-test.route";
    pub const RULE_FILE: &str = "49-jafnify-test.rules";
    pub const RULES_PATH: &str = "/etc/polkit-1/rules.d/49-jafnify-test.rules";
    pub const UNIT: &str = "jafnify-test.service";
}

pub use ids::*;

/// True in the side-by-side test build.
pub const IS_TEST: bool = cfg!(feature = "test-flavor");

/// Apply the flavor rule to a fixed script or embedded text. The product
/// build returns the text unchanged. `jafn0` contains no `jafnify` and is
/// never renamed.
pub fn flavored(text: &str) -> String {
    if IS_TEST {
        text.replace("jafnify", "jafnify-test")
            .replace("Name=Jafnify", "Name=Jafnify Test")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule_matches_the_constants() {
        assert_eq!(flavored("/usr/libexec/jafnify/jafnify-helper"), HELPER_PATH);
        assert_eq!(flavored("/usr/libexec/jafnify/jafnify-route"), ROUTE_PATH);
        assert_eq!(flavored("org.jafnify.tunnel"), ACTION_TUNNEL);
        assert_eq!(flavored("org.jafnify.route"), ACTION_ROUTE);
        assert_eq!(flavored("49-jafnify.rules"), RULE_FILE);
        assert_eq!(
            flavored("/etc/polkit-1/rules.d/49-jafnify.rules"),
            RULES_PATH
        );
        assert_eq!(flavored("jafnify.service"), UNIT);
        assert_eq!(flavored("jafnify"), NAME);
        // The tunnel interface and its config are shared on purpose.
        assert_eq!(
            flavored("/etc/wireguard/jafn0.conf"),
            "/etc/wireguard/jafn0.conf"
        );
    }
}
