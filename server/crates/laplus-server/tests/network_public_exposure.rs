use std::sync::{Arc, Mutex};

use futures_util::SinkExt;
use laplus_server::public_exposure::{EndpointVerifier, NetworkEndpointVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone, Copy)]
enum DescriptorReply {
    Correct,
    WrongEnvironment,
    Redirect,
    AccessHtml,
    Oversized,
}

struct FakeTlsPeer {
    address: std::net::SocketAddr,
    certificate: Vec<u8>,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeTlsPeer {
    async fn start(reply: DescriptorReply, expected_connections: usize) -> Self {
        let certified = rcgen::generate_simple_self_signed(vec!["laplus.test".into()]).unwrap();
        let certificate = certified.cert.der().to_vec();
        let trusted_certificate = certificate.clone();
        let key = certified.key_pair.serialize_der();
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(certificate.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
            )
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            for connection in 0..expected_connections {
                let (stream, _) = listener.accept().await.unwrap();
                let mut tls = acceptor.accept(stream).await.unwrap();
                assert_eq!(tls.get_ref().1.server_name(), Some("laplus.test"));
                if connection == 2 {
                    let captured = Arc::clone(&captured);
                    let callback = move |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                                         response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                        captured.lock().unwrap().push(format!("{request:?}"));
                        assert_eq!(request.uri().path(), "/api/access/cloudflare/challenge/ws");
                        assert_eq!(request.headers()["authorization"], "Bearer ws-secret");
                        Ok(response)
                    };
                    let mut socket = tokio_tungstenite::accept_hdr_async(tls, callback)
                        .await
                        .unwrap();
                    socket.send(Message::Text("ok".into())).await.unwrap();
                    continue;
                }
                let request = read_http_request(&mut tls).await;
                captured.lock().unwrap().push(request.clone());
                if connection == 0 {
                    write_descriptor(&mut tls, reply).await;
                } else {
                    assert!(request.starts_with("GET /api/access/cloudflare/challenge "));
                    assert!(request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer http-secret"));
                    write_response(&mut tls, "200 OK", "application/json", b"{\"ok\":true}").await;
                }
            }
        });
        Self {
            address,
            certificate: trusted_certificate,
            requests,
            task,
        }
    }
}

#[tokio::test]
async fn production_verifier_completes_pinned_https_and_authenticated_wss_with_sni() {
    let peer = FakeTlsPeer::start(DescriptorReply::Correct, 3).await;
    let verifier = NetworkEndpointVerifier::with_hermetic_network(
        vec![peer.address],
        peer.certificate.clone(),
    );
    let origin = format!("https://laplus.test:{}", peer.address.port());

    verifier
        .verify(&origin, "environment-a", "http-secret", "ws-secret")
        .await
        .unwrap();
    peer.task.await.unwrap();

    let requests = peer.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].contains("/.well-known/t3/environment"));
    assert!(!requests[0].contains("http-secret"));
    assert!(!requests[0].contains("ws-secret"));
    assert!(requests[1]
        .to_ascii_lowercase()
        .contains("authorization: bearer http-secret"));
    assert!(!requests[1].contains("ws-secret"));
    assert!(requests[2].contains("Bearer ws-secret"));
    assert!(!requests[2].contains("http-secret"));
}

#[tokio::test]
async fn production_verifier_refuses_redirects_access_html_wrong_identity_and_large_descriptors() {
    for (reply, expected) in [
        (DescriptorReply::Redirect, "cloudflare-access"),
        (DescriptorReply::AccessHtml, "cloudflare-access"),
        (DescriptorReply::WrongEnvironment, "wrong-environment"),
        (DescriptorReply::Oversized, "identity"),
    ] {
        let peer = FakeTlsPeer::start(reply, 1).await;
        let verifier = NetworkEndpointVerifier::with_hermetic_network(
            vec![peer.address],
            peer.certificate.clone(),
        );
        let origin = format!("https://laplus.test:{}", peer.address.port());
        let failure = verifier
            .verify(&origin, "environment-a", "http-secret", "ws-secret")
            .await
            .unwrap_err();
        assert_eq!(failure.kind, expected);
        peer.task.await.unwrap();
        let wire = peer.requests.lock().unwrap().join("\n");
        assert!(!wire.contains("http-secret"));
        assert!(!wire.contains("ws-secret"));
    }
}

async fn read_http_request<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> String {
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await.unwrap();
        bytes.push(byte[0]);
    }
    String::from_utf8(bytes).unwrap()
}

async fn write_descriptor<S: tokio::io::AsyncWrite + Unpin>(
    stream: &mut S,
    reply: DescriptorReply,
) {
    match reply {
        DescriptorReply::Correct => {
            write_response(
                stream,
                "200 OK",
                "application/json",
                b"{\"environmentId\":\"environment-a\"}",
            )
            .await
        }
        DescriptorReply::WrongEnvironment => {
            write_response(
                stream,
                "200 OK",
                "application/json",
                b"{\"environmentId\":\"environment-b\"}",
            )
            .await
        }
        DescriptorReply::Redirect => write_response(stream, "302 Found", "text/plain", b"").await,
        DescriptorReply::AccessHtml => {
            write_response(stream, "200 OK", "text/html", b"<html>Access</html>").await
        }
        DescriptorReply::Oversized => {
            let body = vec![b'x'; 64 * 1024 + 1];
            write_response(stream, "200 OK", "application/json", &body).await;
        }
    }
}

async fn write_response<S: tokio::io::AsyncWrite + Unpin>(
    stream: &mut S,
    status: &str,
    content_type: &str,
    body: &[u8],
) {
    stream.write_all(format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len(),
    ).as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
}
