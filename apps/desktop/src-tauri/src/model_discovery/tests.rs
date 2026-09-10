use super::*;
use crate::domain::ModelSpec;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

const OPENAI: &[u8] = include_bytes!("../../tests/fixtures/models/openai-list-v1.json");
const ANTHROPIC: &[u8] = include_bytes!("../../tests/fixtures/models/anthropic-list-v1.json");
const LAST: &[u8] = include_bytes!("../../tests/fixtures/models/anthropic-last-v1.json");

fn input(url: &str) -> DiscoveryInput {
    DiscoveryInput {
        connection_id: None,
        base_url: url.into(),
        provider_kind: ProviderKind::Openai,
        protocol: ApiProtocol::ChatCompletions,
        headers: BTreeMap::new(),
        api_key: None,
        clear_secret: false,
    }
}

#[test]
fn endpoints_keep_origin_and_prefix_and_reject_unsafe_inputs() {
    for (base, expected) in [
        ("", "/v1/models"),
        ("/", "/v1/models"),
        ("/chat/completions", "/models"),
        ("/v1/", "/v1/models"),
        ("/api/v1/chat/completions", "/api/v1/models"),
        ("/api/v1/responses", "/api/v1/models"),
        ("/v1/messages", "/v1/models"),
        ("/custom/models/", "/custom/models"),
    ] {
        assert_eq!(
            endpoint(&input(&format!("https://example.invalid{base}")))
                .unwrap()
                .as_str(),
            format!("https://example.invalid{expected}")
        );
    }
    for base in [
        "http://example.invalid/v1",
        "https://u:p@example.invalid/v1",
        "https://example.invalid/v1?key=secret",
        "https://example.invalid/#frag",
        "file:///tmp/models",
    ] {
        assert_eq!(endpoint(&input(base)).unwrap_err(), Status::InvalidInput);
    }
    assert!(endpoint(&input("http://127.0.0.1:3210/v1")).is_ok());
    let mut request = input("https://example.invalid/v1");
    request.provider_kind = ProviderKind::Azure;
    assert_eq!(endpoint(&request).unwrap_err(), Status::Unsupported);
    request.provider_kind = ProviderKind::Custom;
    for (name, value) in [
        ("Host", "other.invalid"),
        ("Cookie", "private"),
        ("Connection", "Authorization"),
        ("Proxy-Authorization", "Bearer ${apiKey}"),
        ("Authorization", "literal-key"),
        ("X-Test", "a\r\nb"),
    ] {
        request.headers = BTreeMap::from([(name.into(), value.into())]);
        assert_eq!(endpoint(&request).unwrap_err(), Status::InvalidInput);
    }
    request.headers.clear();
    request.api_key = Some("x\ny".into());
    assert_eq!(endpoint(&request).unwrap_err(), Status::InvalidInput);
}

#[test]
fn credentials_are_bound_to_saved_configuration_and_never_read_when_replaced() {
    let mut request = input("https://example.invalid/v1");
    let saved = Connection {
        id: "fixture".into(),
        name: "Fixture".into(),
        base_url: request.base_url.clone(),
        provider_kind: request.provider_kind,
        protocol: request.protocol,
        headers: BTreeMap::new(),
        models: vec![ModelSpec {
            id: "m".into(),
            model_id: "m".into(),
            name: "m".into(),
            enabled: true,
            capabilities: Default::default(),
        }],
        secret_ref: "connection:fixture".into(),
        has_secret: true,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    assert_eq!(
        credential(&request, Some(&saved), |reference| {
            assert_eq!(reference, "connection:fixture");
            Ok(Some("synthetic-key".into()))
        })
        .unwrap()
        .as_deref(),
        Some("synthetic-key")
    );
    for field in ["host", "path", "protocol", "provider", "headers"] {
        let mut changed = request.clone();
        match field {
            "host" => changed.base_url = "https://elsewhere.invalid/v1".into(),
            "path" => changed.base_url.push_str("/other"),
            "protocol" => changed.protocol = ApiProtocol::Responses,
            "provider" => changed.provider_kind = ProviderKind::Custom,
            _ => {
                changed.headers.insert("X-Route".into(), "elsewhere".into());
            }
        }
        assert_eq!(
            credential(&changed, Some(&saved), |_| panic!(
                "must not read saved key"
            ))
            .unwrap_err(),
            Status::CredentialRequired
        );
    }
    assert_eq!(
        credential(&request, Some(&saved), |_| Ok(None)).unwrap_err(),
        Status::CredentialUnavailable
    );
    request.api_key = Some("replacement".into());
    assert_eq!(
        credential(&request, Some(&saved), |_| panic!(
            "must not read saved key"
        ))
        .unwrap()
        .as_deref(),
        Some("replacement")
    );
    request.api_key = None;
    request.clear_secret = true;
    assert_eq!(
        credential(&request, Some(&saved), |_| panic!(
            "must not read saved key"
        ))
        .unwrap(),
        None
    );
}

#[test]
fn protocol_headers_and_templates_are_native_owned() {
    let mut request = input("https://example.invalid/v1");
    assert_eq!(
        request_headers(&request, Some("fixture")).unwrap()["authorization"],
        "Bearer fixture"
    );
    request.protocol = ApiProtocol::Messages;
    let headers = request_headers(&request, Some("fixture")).unwrap();
    assert_eq!(headers["x-api-key"], "fixture");
    assert_eq!(headers["anthropic-version"], "2023-06-01");
    request
        .headers
        .insert("Authorization".into(), "Token ${apiKey}".into());
    assert_eq!(
        request_headers(&request, Some("fixture")).unwrap()["authorization"],
        "Token fixture"
    );
    assert_eq!(
        request_headers(&request, None).unwrap_err(),
        Status::CredentialRequired
    );
}

#[test]
fn versioned_catalogs_preserve_ids_deduplicate_and_require_known_schema() {
    let page = parse(OPENAI, ApiProtocol::ChatCompletions, None).unwrap();
    assert_eq!(page.models.len(), 2);
    assert_eq!(page.models[0].model_id, "fixture/chat");
    assert_eq!(page.models[1].name, "Fixture Reasoning");
    assert_eq!(
        parse(ANTHROPIC, ApiProtocol::Messages, None)
            .unwrap()
            .next
            .as_deref(),
        Some("fixture-claude-a")
    );
    assert!(parse(LAST, ApiProtocol::Messages, None)
        .unwrap()
        .next
        .is_none());
    for bytes in [
        br#"{}"#.as_slice(),
        br#"{"data":[{"name":"no id"}]}"#,
        br#"{"data":[{"id":"a\nb"}]}"#,
        br#"{"data":[{"id":"a|b"}]}"#,
        br#"{"data":[{"id":"safe","name":"PRIVATE_KEY"}]}"#,
        b"not json PRIVATE_KEY",
    ] {
        assert!(matches!(
            parse(bytes, ApiProtocol::Responses, Some("PRIVATE_KEY")),
            Err(Status::SchemaError)
        ));
    }
    assert!(matches!(
        parse(OPENAI, ApiProtocol::Messages, None),
        Err(Status::SchemaError)
    ));
    assert!(
        parse(
            br#"{"data":[],"has_more":true}"#,
            ApiProtocol::Responses,
            None
        )
        .unwrap()
        .partial
    );
    let oversized =
        serde_json::to_vec(&serde_json::json!({"data":[{"id":"x".repeat(321)}]})).unwrap();
    assert!(matches!(
        parse(&oversized, ApiProtocol::Responses, None),
        Err(Status::SchemaError)
    ));
    assert!(matches!(
        parse(
            &vec![b' '; MAX_BYTES as usize + 1],
            ApiProtocol::Responses,
            None
        ),
        Err(Status::SchemaError)
    ));
    let many = serde_json::to_vec(&serde_json::json!({"data":(0..2049).map(|i| serde_json::json!({"id":format!("model-{i}")})).collect::<Vec<_>>()})).unwrap();
    let page = parse(&many, ApiProtocol::Responses, None).unwrap();
    assert_eq!(page.models.len(), 2048);
    assert!(page.partial);
}

fn server(
    responses: Vec<(u16, Vec<u8>, &'static str)>,
) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = format!("http://{}/v1", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        for (code, body, extra) in responses {
            let end = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < end =>
                    {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("fixture did not receive request: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 16384);
            }
            requests.push(String::from_utf8(request).unwrap());
            write!(
                stream,
                "HTTP/1.1 {code} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        }
        requests
    });
    (address, handle)
}

#[test]
fn real_http_get_follows_bounded_anthropic_cursors_on_same_origin() {
    let (url, server) = server(vec![
        (200, ANTHROPIC.to_vec(), ""),
        (200, LAST.to_vec(), ""),
    ]);
    let mut request = input(&url);
    request.protocol = ApiProtocol::Messages;
    let result = fetch(
        &request,
        endpoint(&request).unwrap(),
        request_headers(&request, Some("synthetic-key")).unwrap(),
        Some("synthetic-key"),
    );
    assert_eq!(result.status, Status::Available);
    assert_eq!(result.models.len(), 2);
    let requests = server.join().unwrap();
    assert!(requests[0].starts_with("GET /v1/models?limit=1000 "));
    assert!(requests[0]
        .to_lowercase()
        .contains("x-api-key: synthetic-key\r\n"));
    assert!(requests[1].starts_with("GET /v1/models?limit=1000&after_id=fixture-claude-a "));
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("synthetic-key"));
}

#[test]
fn real_http_errors_redirects_empty_and_partial_never_become_fake_success() {
    for (code, body, extra, expected) in [
        (401, b"PRIVATE_KEY".to_vec(), "", Status::Unauthorized),
        (403, Vec::new(), "", Status::Unauthorized),
        (429, Vec::new(), "", Status::RateLimited),
        (
            302,
            Vec::new(),
            "Location: http://127.0.0.1:1/steal\r\n",
            Status::Unsupported,
        ),
        (200, b"{\"data\":[]}".to_vec(), "", Status::Empty),
        (200, b"PRIVATE_KEY".to_vec(), "", Status::SchemaError),
    ] {
        let (url, server) = server(vec![(code, body, extra)]);
        let request = input(&url);
        let result = fetch(
            &request,
            endpoint(&request).unwrap(),
            request_headers(&request, Some("PRIVATE_KEY")).unwrap(),
            Some("PRIVATE_KEY"),
        );
        assert_eq!(result.status, expected);
        assert_eq!(server.join().unwrap().len(), 1);
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("PRIVATE_KEY"));
    }
    let (url, server) = server(vec![(200, ANTHROPIC.to_vec(), ""), (500, Vec::new(), "")]);
    let mut request = input(&url);
    request.protocol = ApiProtocol::Messages;
    let result = fetch(&request, endpoint(&request).unwrap(), BTreeMap::new(), None);
    assert_eq!(result.status, Status::Partial);
    assert_eq!(result.models.len(), 1);
    server.join().unwrap();
}

#[test]
fn single_flight_is_released_on_drop() {
    let flight = Flight::claim().unwrap();
    assert!(Flight::claim().is_none());
    drop(flight);
    assert!(Flight::claim().is_some());
}

#[test]
fn saved_key_read_excludes_cross_process_updates_and_rechecks_disk_state() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("state.json");
    let mut state = crate::domain::PersistentState::default();
    state.connections.push(Connection {
        id: "fixture".into(),
        name: "Fixture".into(),
        base_url: "https://example.invalid/v1".into(),
        provider_kind: ProviderKind::Openai,
        protocol: ApiProtocol::ChatCompletions,
        headers: BTreeMap::new(),
        models: vec![ModelSpec {
            id: "m".into(),
            model_id: "m".into(),
            name: "m".into(),
            enabled: true,
            capabilities: Default::default(),
        }],
        secret_ref: "connection:fixture".into(),
        has_secret: true,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    });
    std::fs::write(&file, serde_json::to_vec(&state).unwrap()).unwrap();
    let store = crate::state::StateStore::open_at(file.clone());
    let mut request = input("https://example.invalid/v1");
    request.connection_id = Some("fixture".into());
    let key = key_from_store(&store, &request, |_| {
        assert!(crate::write_lock::WriteLock::acquire(&file).is_err());
        Ok(Some("synthetic".into()))
    })
    .unwrap();
    assert_eq!(key.as_deref(), Some("synthetic"));
    let other = crate::write_lock::WriteLock::acquire(&file).unwrap();
    assert_eq!(
        key_from_store(&store, &request, |_| panic!(
            "must not read during another write"
        ))
        .unwrap_err(),
        Status::Busy
    );
    drop(other);
    state.connections[0].base_url = "https://elsewhere.invalid/v1".into();
    std::fs::write(&file, serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(
        key_from_store(&store, &request, |_| panic!(
            "must not read from stale state"
        ))
        .unwrap_err(),
        Status::CredentialUnavailable
    );
}

#[test]
fn fixed_error_messages_survive_central_redaction() {
    for status in [
        Status::InvalidInput,
        Status::Unauthorized,
        Status::CredentialRequired,
        Status::CredentialUnavailable,
        Status::SchemaError,
        Status::NetworkError,
    ] {
        assert_eq!(DiscoveryResult::error(status).detail, status.detail());
    }
}
