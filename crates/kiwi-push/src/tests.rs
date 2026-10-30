use std::collections::HashMap;

use p256::ecdsa::VerifyingKey;
use p256::ecdsa::signature::Verifier;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;

fn b64(s: &str) -> Vec<u8> {
    decode_b64(s).unwrap()
}

// RFC 8291 Appendix A / Section 5.
const AS_PRIVATE: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
const AS_PUBLIC: &str =
    "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
const UA_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
const UA_PUBLIC: &str =
    "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
const RESULT: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml\
                      mlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPT\
                      pK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";

#[test]
fn rfc8291_appendix_a_vector() {
    let as_secret = SecretKey::from_slice(&b64(AS_PRIVATE)).unwrap();
    assert_eq!(
        as_secret.public_key().to_sec1_point(false).as_bytes(),
        b64(AS_PUBLIC).as_slice()
    );
    let salt: [u8; 16] = b64(SALT).try_into().unwrap();
    let body = encrypt(
        &b64(UA_PUBLIC),
        &b64(AUTH),
        b"When I grow up, I want to be a watermelon",
        &as_secret,
        salt,
    )
    .unwrap();
    assert_eq!(B64.encode(&body), RESULT);
    // 86-byte header + 41 + 1 delimiter + 16 tag (the RFC's `Content-Length: 145` is off by one).
    assert_eq!(body.len(), 144);
}

/// Receiver side of RFC 8291, used to check what `send` puts on the wire.
fn decrypt(ua_secret: &SecretKey, auth: &[u8], body: &[u8]) -> Vec<u8> {
    let salt = &body[..16];
    assert_eq!(
        u32::from_be_bytes(body[16..20].try_into().unwrap()),
        RECORD_SIZE
    );
    assert_eq!(body[20], 65);
    let as_public = &body[21..86];
    let ua_public = ua_secret.public_key().to_sec1_point(false);
    let shared = ua_secret.diffie_hellman(&PublicKey::from_sec1_bytes(as_public).unwrap());
    let info = [
        b"WebPush: info\0".as_slice(),
        ua_public.as_bytes(),
        as_public,
    ]
    .concat();
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes())
        .expand(&info, &mut ikm)
        .unwrap();
    let prk = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let (mut cek, mut nonce) = ([0u8; 16], [0u8; 12]);
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .unwrap();
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce)
        .unwrap();
    let mut plain = Aes128Gcm::new_from_slice(&cek)
        .unwrap()
        .decrypt(&nonce.into(), &body[86..])
        .unwrap();
    assert_eq!(plain.pop(), Some(2), "last-record padding delimiter");
    plain
}

#[test]
fn rfc8291_vector_decrypts() {
    let ua_secret = SecretKey::from_slice(&b64(UA_PRIVATE)).unwrap();
    let plain = decrypt(&ua_secret, &b64(AUTH), &b64(RESULT));
    assert_eq!(plain, b"When I grow up, I want to be a watermelon");
}

#[test]
fn payload_limits_and_key_validation() {
    let as_secret = SecretKey::from_slice(&b64(AS_PRIVATE)).unwrap();
    let (ua, auth) = (b64(UA_PUBLIC), b64(AUTH));
    let max = encrypt(&ua, &auth, &[b'x'; MAX_PAYLOAD], &as_secret, [0; 16]).unwrap();
    assert_eq!(max.len(), 4096);
    assert!(matches!(
        encrypt(&ua, &auth, &[b'x'; MAX_PAYLOAD + 1], &as_secret, [0; 16]),
        Err(Error::PayloadTooLarge(_))
    ));
    assert!(matches!(
        encrypt(&ua[..33], &auth, b"x", &as_secret, [0; 16]),
        Err(Error::InvalidSubscription(_))
    ));
    assert!(matches!(
        encrypt(&ua, &auth[..8], b"x", &as_secret, [0; 16]),
        Err(Error::InvalidSubscription(_))
    ));
    let mut bad_point = ua.clone();
    bad_point[64] ^= 1;
    assert!(encrypt(&bad_point, &auth, b"x", &as_secret, [0; 16]).is_err());
}

/// Splits `vapid t=<jwt>, k=<key>`, verifies the ES256 signature and returns the claims.
fn verify_authorization(header: &str, vapid: &Vapid) -> serde_json::Value {
    let rest = header.strip_prefix("vapid t=").expect("vapid scheme");
    let (jwt, key) = rest.split_once(", k=").expect("k parameter");
    assert_eq!(key, vapid.public_key());
    let verifying = VerifyingKey::from_sec1_bytes(&b64(key)).unwrap();
    let (signing_input, sig) = jwt.rsplit_once('.').unwrap();
    let sig = Signature::from_slice(&b64(sig)).unwrap();
    verifying
        .verify(signing_input.as_bytes(), &sig)
        .expect("valid ES256 signature");
    let (header, claims) = signing_input.split_once('.').unwrap();
    let header: serde_json::Value = serde_json::from_slice(&b64(header)).unwrap();
    assert_eq!(header["alg"], "ES256");
    assert_eq!(header["typ"], "JWT");
    serde_json::from_slice(&b64(claims)).unwrap()
}

#[test]
fn vapid_jwt_round_trip() {
    let vapid = Vapid::generate().unwrap();
    let now = SystemTime::now();
    let header = vapid.authorization("https://web.push.apple.com", "mailto:kiwi@example.com", now);
    let claims = verify_authorization(&header, &vapid);
    assert_eq!(claims["aud"], "https://web.push.apple.com");
    assert_eq!(claims["sub"], "mailto:kiwi@example.com");
    let now = now.duration_since(UNIX_EPOCH).unwrap().as_secs();
    let exp = claims["exp"].as_u64().unwrap();
    assert!(exp > now && exp <= now + 12 * 3600);

    // A different key must not verify.
    let other = Vapid::generate().unwrap();
    let (input, sig) = header["vapid t=".len()..]
        .split_once(", k=")
        .unwrap()
        .0
        .rsplit_once('.')
        .unwrap();
    let sig = Signature::from_slice(&b64(sig)).unwrap();
    assert!(
        other
            .key
            .verifying_key()
            .verify(input.as_bytes(), &sig)
            .is_err()
    );
}

#[test]
fn vapid_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub/vapid.json");
    let created = Vapid::load_or_create(&path).unwrap();
    let key = b64(&created.public_key());
    assert_eq!(key.len(), 65);
    assert_eq!(key[0], 0x04);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let loaded = Vapid::load_or_create(&path).unwrap();
    assert_eq!(loaded.public_key(), created.public_key());
    assert!(!format!("{loaded:?}").contains(&B64.encode(loaded.key.to_bytes())));
    // Only the key file is left behind.
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );

    std::fs::write(&path, "{\"private_key\":\"AAAA\",\"public_key\":\"x\"}").unwrap();
    assert!(matches!(
        Vapid::load_or_create(&path),
        Err(Error::KeyFile(_))
    ));
    std::fs::write(&path, "not json").unwrap();
    assert!(matches!(
        Vapid::load_or_create(&path),
        Err(Error::KeyFile(_))
    ));
    let other = Vapid::generate().unwrap();
    let mismatched = serde_json::json!({
        "private_key": B64.encode(created.key.to_bytes()),
        "public_key": other.public_key(),
    });
    std::fs::write(&path, mismatched.to_string()).unwrap();
    assert!(matches!(
        Vapid::load_or_create(&path),
        Err(Error::KeyFile(_))
    ));
}

#[test]
fn endpoints() {
    for ok in [
        "https://web.push.apple.com/QGuQyavXutnMH4mEhFGXzK...",
        "https://api.push.apple.com/3/device/abc",
        "https://fcm.googleapis.com/fcm/send/abc:def",
        "https://fcm.googleapis.com/wp/abc",
        "https://updates.push.services.mozilla.com/wpush/v2/gAAAA",
        "https://foo.push.services.mozilla.com/wpush/v1/x",
        "https://wns2-par02p.notify.windows.com/w/?token=BQYAAA",
        "https://WEB.PUSH.APPLE.COM/x",
        "https://web.push.apple.com:443/x",
    ] {
        assert!(endpoint_allowed(ok), "{ok}");
    }
    for bad in [
        "",
        "not a url",
        "http://web.push.apple.com/x",
        "wss://web.push.apple.com/x",
        "https://web.push.apple.com:8443/x",
        "https://user:pw@web.push.apple.com/x",
        "https://push.apple.com/x",
        "https://evilpush.apple.com/x",
        "https://web.push.apple.com.evil.example/x",
        "https://web.push.apple.com./x",
        "https://fcm.googleapis.com.evil.example/x",
        "https://evil.googleapis.com/x",
        "https://mozilla.com/x",
        "https://notify.windows.com/x",
        "https://evil-notify.windows.com/x",
        "https://127.0.0.1/x",
        "https://[::1]/x",
        "https://localhost/x",
    ] {
        assert!(!endpoint_allowed(bad), "{bad}");
    }
}

#[test]
fn topics_and_subjects() {
    assert_eq!(push_topic("update-123_ABC"), "update-123_ABC");
    let hashed = push_topic("update:com.google.ios.youtube.A1B2C3D4E5");
    assert_eq!(hashed.len(), 32);
    assert!(
        hashed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );
    assert_eq!(
        hashed,
        push_topic("update:com.google.ios.youtube.A1B2C3D4E5")
    );
    assert_eq!(push_topic(&"a".repeat(33)).len(), 32);

    let vapid = Vapid::generate().unwrap();
    for ok in [
        "https://mac-studio.example.ts.net",
        "mailto:alex@example.com",
    ] {
        assert!(PushSender::new(vapid.clone(), ok.into()).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "mac-studio",
        "http://example.com",
        "mailto:",
        "https://",
    ] {
        assert!(PushSender::new(vapid.clone(), bad.into()).is_err(), "{bad}");
    }
    assert_eq!(truncate("héllo".into(), 2), "h…");
    assert_eq!(truncate("hello".into(), 5), "hello");
}

struct Captured {
    request_line: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

/// Minimal one-shot HTTP/1.1 server answering with `status` and `body`.
async fn serve_once(status: u16, body: String) -> (String, tokio::task::JoinHandle<Captured>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let header_end = loop {
            let mut chunk = [0u8; 4096];
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0, "connection closed before headers");
            buf.extend_from_slice(&chunk[..n]);
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
        };
        let head = String::from_utf8(buf[..header_end].to_vec()).unwrap();
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap().to_owned();
        let headers: HashMap<_, _> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
            .collect();
        let len: usize = headers["content-length"].parse().unwrap();
        while buf.len() < header_end + len {
            let mut chunk = [0u8; 4096];
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0, "connection closed before body");
            buf.extend_from_slice(&chunk[..n]);
        }
        let response = format!(
            "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap();
        Captured {
            request_line,
            headers,
            body: buf[header_end..].to_vec(),
        }
    });
    (base, handle)
}

fn subscription(endpoint: String) -> (Subscription, SecretKey, Vec<u8>) {
    let ua_secret = SecretKey::try_generate().unwrap();
    let auth = vec![7u8; 16];
    let sub = Subscription {
        endpoint,
        p256dh: B64.encode(ua_secret.public_key().to_sec1_point(false).as_bytes()),
        auth: B64.encode(&auth),
    };
    (sub, ua_secret, auth)
}

#[tokio::test]
async fn send_posts_encrypted_payload_with_headers() {
    let (base, server) = serve_once(201, String::new()).await;
    let (sub, ua_secret, auth) = subscription(format!("{base}/push/abc?x=1"));
    let sender =
        PushSender::new(Vapid::generate().unwrap(), "mailto:k@example.com".into()).unwrap();
    let payload = br#"{"title":"Kiwi","body":"ready"}"#;
    let outcome = sender
        .deliver(
            &sub,
            payload,
            3600,
            Urgency::High,
            Some("update:com.example.app"),
        )
        .await
        .unwrap();
    assert_eq!(outcome, Outcome::Delivered);

    let req = server.await.unwrap();
    assert_eq!(req.request_line, "POST /push/abc?x=1 HTTP/1.1");
    let h = &req.headers;
    assert_eq!(h["ttl"], "3600");
    assert_eq!(h["urgency"], "high");
    assert_eq!(h["topic"], push_topic("update:com.example.app"));
    assert_eq!(h["content-encoding"], "aes128gcm");
    assert_eq!(h["content-type"], "application/octet-stream");
    let claims = verify_authorization(&h["authorization"], sender.vapid());
    assert_eq!(claims["aud"], base);
    assert_eq!(claims["sub"], "mailto:k@example.com");
    assert_eq!(decrypt(&ua_secret, &auth, &req.body), payload);
}

#[tokio::test]
async fn send_maps_status_codes() {
    let sender =
        PushSender::new(Vapid::generate().unwrap(), "https://kiwi.example".into()).unwrap();
    let long = "e".repeat(2000);
    for (status, body, expected) in [
        (200, String::new(), Outcome::Delivered),
        (202, String::new(), Outcome::Delivered),
        (404, String::new(), Outcome::Gone),
        (410, "{\"reason\":\"Unregistered\"}".into(), Outcome::Gone),
        (
            403,
            "{\"reason\":\"BadJwtToken\"}".into(),
            Outcome::Rejected {
                status: 403,
                body: "{\"reason\":\"BadJwtToken\"}".into(),
            },
        ),
        (
            413,
            long.clone(),
            Outcome::Rejected {
                status: 413,
                body: format!("{}…", &long[..MAX_BODY_IN_OUTCOME]),
            },
        ),
        (
            500,
            format!("{}€end", "x".repeat(MAX_BODY_IN_OUTCOME - 1)),
            Outcome::Rejected {
                status: 500,
                body: format!("{}…", "x".repeat(MAX_BODY_IN_OUTCOME - 1)),
            },
        ),
        (
            503,
            "x".repeat(MAX_BODY_IN_OUTCOME),
            Outcome::Rejected {
                status: 503,
                body: "x".repeat(MAX_BODY_IN_OUTCOME),
            },
        ),
        (
            301,
            String::new(),
            Outcome::Rejected {
                status: 301,
                body: String::new(),
            },
        ),
    ] {
        let (base, server) = serve_once(status, body).await;
        let (sub, _, _) = subscription(format!("{base}/p"));
        let outcome = sender
            .deliver(&sub, b"hi", 60, Urgency::Normal, None)
            .await
            .unwrap();
        assert_eq!(outcome, expected, "status {status}");
        let req = server.await.unwrap();
        assert!(!req.headers.contains_key("topic"));
        assert_eq!(req.headers["urgency"], "normal");
    }
}

#[tokio::test]
async fn rejection_body_stops_after_the_prefix_without_waiting_for_eof() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 4096];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 503 X\r\nTransfer-Encoding: chunked\r\n\r\n")
            .await
            .unwrap();
        let prefix = "x".repeat(MAX_BODY_IN_OUTCOME + 1);
        socket
            .write_all(format!("{:x}\r\n{prefix}\r\n", prefix.len()).as_bytes())
            .await
            .unwrap();
        // Deliberately leave the response unfinished; the reader must close it itself.
        let _ = socket.read(&mut request).await;
    });
    let response = reqwest::Client::new()
        .get(endpoint)
        .timeout(Duration::from_secs(1))
        .send()
        .await
        .unwrap();
    assert_eq!(
        rejection_body(response).await,
        format!("{}…", "x".repeat(MAX_BODY_IN_OUTCOME))
    );
    server.await.unwrap();
}

#[tokio::test]
async fn send_rejects_disallowed_endpoint_and_bad_keys() {
    let sender =
        PushSender::new(Vapid::generate().unwrap(), "https://kiwi.example".into()).unwrap();
    let (sub, _, _) = subscription("http://127.0.0.1:9/p".into());
    assert!(matches!(
        sender.send(&sub, b"x", 60, Urgency::Low, None).await,
        Err(Error::EndpointNotAllowed)
    ));
    let (mut sub, _, _) = subscription("https://web.push.apple.com/x".into());
    sub.auth = "!!".into();
    assert!(matches!(
        sender.send(&sub, b"x", 60, Urgency::Low, None).await,
        Err(Error::InvalidSubscription(_))
    ));
}

#[test]
fn base64_variants() {
    assert_eq!(
        decode_b64("BTBZMqHH6r4Tts7J_aSIgg"),
        decode_b64("BTBZMqHH6r4Tts7J/aSIgg==")
    );
    assert!(decode_b64("@@").is_none());
}
