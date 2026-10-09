//! OAuth 1.0 HMAC-SHA1 validation with explicitly local replay/window policies.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::net::IpAddr;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Config {
    pub base_url: String,
    consumer_key: String,
    consumer_secret: String,
    token: String,
    token_secret: String,
    #[serde(default = "window")]
    timestamp_window_seconds: i64,
    #[serde(default)]
    allowed_ips: Vec<IpAddr>,
}
fn window() -> i64 {
    300
}
fn rejected(message: &str) -> ApiError {
    ApiError(
        StatusCode::UNAUTHORIZED,
        "BAD_OAUTH_REQUEST",
        message.into(),
    )
}
fn denied() -> ApiError {
    ApiError(
        StatusCode::FORBIDDEN,
        "PERMISSION_DENIED",
        "Peer IP is not allowed by local validation policy".into(),
    )
}
pub(super) fn encode(value: &str) -> String {
    let mut result = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            result.push(b as char);
        } else {
            result.push_str(&format!("%{b:02X}"));
        }
    }
    result
}
fn decode(value: &str) -> Result<String, ApiError> {
    let mut result = Vec::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .ok_or_else(|| rejected("Malformed OAuth percent encoding"))?;
            let hi = (hex[0] as char)
                .to_digit(16)
                .ok_or_else(|| rejected("Malformed OAuth percent encoding"))?;
            let lo = (hex[1] as char)
                .to_digit(16)
                .ok_or_else(|| rejected("Malformed OAuth percent encoding"))?;
            result.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(result).map_err(|_| rejected("OAuth values must be UTF-8"))
}
struct OAuthObject(BTreeMap<String, String>);
impl<'de> Deserialize<'de> for OAuthObject {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Unique;
        impl<'de> serde::de::Visitor<'de> for Unique {
            type Value = OAuthObject;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("unique OAuth string fields")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut fields = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, String>()? {
                    if fields.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate OAuth field"));
                    }
                }
                Ok(OAuthObject(fields))
            }
        }
        deserializer.deserialize_map(Unique)
    }
}
impl Config {
    pub(super) fn validate(&self) -> Result<(), ApiError> {
        let url = url::Url::parse(&self.base_url).map_err(|_| invalid("Invalid OAuth base_url"))?;
        if !["http", "https"].contains(&url.scheme())
            || url.host_str().is_none()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(invalid(
                "OAuth base_url must be an HTTP(S) origin without path, credentials, query or fragment",
            ));
        }
        if self.consumer_key.is_empty()
            || self.consumer_secret.is_empty()
            || self.token.is_empty()
            || self.token_secret.is_empty()
            || !(1..=86400).contains(&self.timestamp_window_seconds)
        {
            return Err(invalid(
                "Supply nonempty dummy OAuth credentials and a 1..86400 second window",
            ));
        }
        Ok(())
    }
    pub(super) fn check(
        &self,
        parts: &axum::http::request::Parts,
        bytes: &[u8],
        now: i64,
        nonces: &mut BTreeMap<String, i64>,
    ) -> Result<(), ApiError> {
        if !self.allowed_ips.is_empty() {
            let peer = parts
                .extensions
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|p| p.0.ip());
            if !peer.is_some_and(|p| self.allowed_ips.contains(&p)) {
                return Err(denied());
            }
        }
        decode(parts.uri.query().unwrap_or(""))?;
        let query: Vec<(String, String)> =
            url::form_urlencoded::parse(parts.uri.query().unwrap_or("").as_bytes())
                .into_owned()
                .collect();
        let query_auth: Vec<_> = query.iter().filter(|(k, _)| k == "Authorization").collect();
        let headers: Vec<_> = parts.headers.get_all("authorization").iter().collect();
        if headers.len() + query_auth.len() != 1 {
            return Err(rejected(
                "Supply exactly one Authorization header or JSON query parameter",
            ));
        }
        let mut oauth = BTreeMap::new();
        let fields: Vec<(String, String)> = if let Some(header) = headers.first() {
            let header = header
                .to_str()
                .map_err(|_| rejected("Invalid Authorization header"))?;
            let content = header
                .strip_prefix("OAuth ")
                .ok_or_else(|| rejected("Expected OAuth Authorization scheme"))?;
            content
                .split(',')
                .map(|part| {
                    let (key, value) = part
                        .trim()
                        .split_once('=')
                        .ok_or_else(|| rejected("Malformed OAuth header"))?;
                    let value = value
                        .strip_prefix('"')
                        .and_then(|s| s.strip_suffix('"'))
                        .ok_or_else(|| rejected("OAuth header values must be quoted"))?;
                    Ok((decode(key)?, decode(value)?))
                })
                .collect::<Result<_, ApiError>>()?
        } else {
            // BrickLink wraps percent-escaped OAuth values in a URL-encoded JSON object.
            let object: OAuthObject = serde_json::from_str(&query_auth[0].1)
                .map_err(|_| rejected("Malformed or duplicate Authorization JSON"))?;
            object
                .0
                .iter()
                .map(|(k, v)| Ok((decode(k)?, decode(v)?)))
                .collect::<Result<_, ApiError>>()?
        };
        for (key, value) in fields {
            if key == "realm" {
                continue;
            }
            if !key.starts_with("oauth_") || oauth.insert(key, value).is_some() {
                return Err(rejected("Unexpected or duplicate OAuth parameter"));
            }
        }
        let get = |key: &str| {
            oauth
                .get(key)
                .map(String::as_str)
                .ok_or_else(|| rejected("Missing OAuth parameter"))
        };
        if oauth.len() != 7
            || get("oauth_version")? != "1.0"
            || get("oauth_signature_method")? != "HMAC-SHA1"
        {
            return Err(rejected("Expected OAuth 1.0 HMAC-SHA1 parameters"));
        }
        if get("oauth_consumer_key")? != self.consumer_key || get("oauth_token")? != self.token {
            return Err(rejected("Unknown dummy consumer or token"));
        }
        let timestamp = get("oauth_timestamp")?
            .parse::<i64>()
            .map_err(|_| rejected("Invalid OAuth timestamp"))?;
        if timestamp < 0 || timestamp.abs_diff(now) > self.timestamp_window_seconds as u64 {
            return Err(rejected("Timestamp outside local acceptance window"));
        }
        let nonce = get("oauth_nonce")?;
        if nonce.is_empty() || nonce.len() > 256 {
            return Err(rejected("Invalid OAuth nonce"));
        }
        nonces.retain(|_, expires| *expires >= now);
        if nonces.contains_key(nonce) {
            return Err(rejected("OAuth nonce already used"));
        }
        let origin = url::Url::parse(&self.base_url)
            .expect("validated origin")
            .origin()
            .ascii_serialization();
        let base_url = format!("{origin}{}", parts.uri.path());
        let mut signing: Vec<(String, String)> = query
            .into_iter()
            .filter(|(k, _)| k != "Authorization")
            .collect();
        if signing.iter().any(|(k, _)| k.starts_with("oauth_")) {
            return Err(rejected(
                "OAuth query fields must use the documented Authorization JSON wrapper",
            ));
        }
        // JSON and raw percent-encoded JSON are not OAuth form parameter sets.
        // The simulator's data= form extension uses standard RFC form signing.
        if parts
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| s.split(';').next() == Some("application/x-www-form-urlencoded"))
            && bytes.starts_with(b"data=")
        {
            signing.extend(url::form_urlencoded::parse(bytes).into_owned());
        }
        signing.extend(
            oauth
                .iter()
                .filter(|(k, _)| k.as_str() != "oauth_signature")
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        let mut encoded: Vec<_> = signing
            .iter()
            .map(|(k, v)| (encode(k), encode(v)))
            .collect();
        encoded.sort();
        let normalized = encoded
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let base = format!(
            "{}&{}&{}",
            parts.method,
            encode(&base_url),
            encode(&normalized)
        );
        let key = format!(
            "{}&{}",
            encode(&self.consumer_secret),
            encode(&self.token_secret)
        );
        let signature = STANDARD
            .decode(get("oauth_signature")?)
            .map_err(|_| rejected("Invalid signature encoding"))?;
        let mut mac = Hmac::<Sha1>::new_from_slice(key.as_bytes())
            .expect("HMAC accepts arbitrary key length");
        mac.update(base.as_bytes());
        mac.verify_slice(&signature)
            .map_err(|_| rejected("OAuth signature mismatch"))?;
        if nonces.len() >= 10000 {
            return Err(rejected(
                "Local nonce capacity exceeded; advance clock or reset",
            ));
        }
        nonces.insert(
            nonce.into(),
            timestamp.saturating_add(self.timestamp_window_seconds),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn published_oauth_get_vector_checks_independently_of_our_test_signer() {
        let config = Config {
            base_url: "http://photos.example.net".into(),
            consumer_key: "dpf43f3p2l4k3l03".into(),
            consumer_secret: "kd94hf93k423kf44".into(),
            token: "nnch734d00sl2jdk".into(),
            token_secret: "pfkkdhi9sl3r4s00".into(),
            timestamp_window_seconds: 300,
            allowed_ips: vec![],
        };
        let request=axum::http::Request::builder().uri("/photos?file=vacation.jpg&size=original").header("authorization",r#"OAuth oauth_consumer_key="dpf43f3p2l4k3l03", oauth_token="nnch734d00sl2jdk", oauth_nonce="kllo9940pd9333jh", oauth_timestamp="1191242096", oauth_signature_method="HMAC-SHA1", oauth_version="1.0", oauth_signature="tR3%2BTy81lMeYAr%2FFid0kMTYa%2FWM%3D""#).body(()).unwrap();
        config
            .check(
                &request.into_parts().0,
                &[],
                1191242096,
                &mut BTreeMap::new(),
            )
            .unwrap();
    }
}
