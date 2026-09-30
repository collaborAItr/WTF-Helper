//! Which service this build talks to. Chosen at compile time; there is no setting,
//! environment variable or file that points a build at another host.

pub struct Flavor {
    pub product_name: &'static str,
    pub service_name: &'static str,
    pub is_test: bool,
    pub scheme: &'static str,
    pub host: &'static str,
    pub keychain_service: &'static str,
}

#[cfg(all(feature = "test-build", feature = "local-dev"))]
compile_error!("Choose one of the features test-build or local-dev, not both.");

#[cfg(not(any(feature = "test-build", feature = "local-dev")))]
pub const FLAVOR: Flavor = Flavor {
    product_name: "WTF Helper",
    service_name: "collaborAItr",
    is_test: false,
    scheme: "https",
    host: "api.collaboraitr.com",
    keychain_service: "com.collaboraitr.wtfhelper",
};

#[cfg(feature = "test-build")]
pub const FLAVOR: Flavor = Flavor {
    product_name: "WTF Helper Test",
    service_name: "collaborAItr test site",
    is_test: true,
    scheme: "https",
    host: "apidev.collaboraitr.com",
    keychain_service: "com.collaboraitr.wtfhelper.test",
};

#[cfg(feature = "local-dev")]
pub const FLAVOR: Flavor = Flavor {
    product_name: "WTF Helper Local",
    service_name: "local backend",
    is_test: true,
    scheme: "http",
    host: "localhost:3001",
    keychain_service: "com.collaboraitr.wtfhelper.local",
};

impl Flavor {
    pub fn origin(&self) -> String {
        format!("{}://{}", self.scheme, self.host)
    }

    pub fn api_base(&self) -> String {
        format!("{}/api/wtf/helper", self.origin())
    }

    /// True only for URLs on this build's own scheme, host and port.
    pub fn is_pinned(&self, url: &reqwest::Url) -> bool {
        url.origin().ascii_serialization() == self.origin()
    }
}

pub const HELPER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(any(feature = "test-build", feature = "local-dev")))]
    #[test]
    fn official_build_is_pinned_to_production() {
        assert_eq!(
            FLAVOR.api_base(),
            "https://api.collaboraitr.com/api/wtf/helper"
        );
        const { assert!(!FLAVOR.is_test) };
        assert_eq!(FLAVOR.keychain_service, "com.collaboraitr.wtfhelper");
    }

    #[cfg(feature = "test-build")]
    #[test]
    fn test_build_is_pinned_to_apidev() {
        assert_eq!(
            FLAVOR.api_base(),
            "https://apidev.collaboraitr.com/api/wtf/helper"
        );
        const { assert!(FLAVOR.is_test) };
        assert_eq!(FLAVOR.keychain_service, "com.collaboraitr.wtfhelper.test");
    }

    #[test]
    fn other_hosts_are_not_pinned() {
        for other in [
            "https://evil.example/api/wtf/helper",
            "https://api.collaboraitr.com.evil.example/api/wtf/helper",
            "http://api.collaboraitr.com/api/wtf/helper",
            "https://api.collaboraitr.com:8443/api/wtf/helper",
            "https://apidev.collaboraitr.com.example/api",
        ] {
            let url = reqwest::Url::parse(other).unwrap();
            assert!(!FLAVOR.is_pinned(&url), "{other} must not be accepted");
        }
        let own = reqwest::Url::parse(&format!("{}/stream", FLAVOR.api_base())).unwrap();
        assert!(FLAVOR.is_pinned(&own));
    }
}
