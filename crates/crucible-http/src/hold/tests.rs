use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hyper::Method;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::{Hold, Origin};
use crate::{Http, HttpError, Lookups, Outgoing, PlainLookups, ProxyEnv, Tls};

/// Holds one origin while `holding` is set, under the name `route`.
#[derive(Debug)]
struct One {
    origin: Origin,
    holding: AtomicBool,
    asked: Mutex<Vec<Origin>>,
}

impl Hold for One {
    fn held(&self, origin: &Origin) -> Option<Box<str>> {
        self.asked.lock().unwrap().push(origin.clone());
        (*origin == self.origin && self.holding.load(Ordering::SeqCst)).then(|| "route".into())
    }
}

/// A listener on loopback that counts every byte sent to it and answers each
/// connection's first request with an empty 200.
async fn recording() -> (String, Arc<Mutex<usize>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let heard = Arc::new(Mutex::new(0));
    let counting = Arc::clone(&heard);
    tokio::spawn(async move {
        while let Ok((mut tcp, _)) = listener.accept().await {
            let counting = Arc::clone(&counting);
            tokio::spawn(async move {
                let mut buffer = [0; 4096];
                let read = tcp.read(&mut buffer).await.unwrap_or(0);
                *counting.lock().unwrap() += read;
                let _ = tcp
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    (url, heard)
}

fn client(hold: Arc<dyn Hold>) -> Http {
    let lookups: PlainLookups = Lookups::plain(NonZeroUsize::MIN);
    Http::new(
        &Tls::new().unwrap(),
        lookups.clone().into(),
        lookups,
        ProxyEnv::read(|_| None),
    )
    .holding(hold)
}

async fn post(http: &Http, url: &str) -> Result<u16, HttpError> {
    let mut headers = Outgoing::new();
    let sent = http.send(Method::POST, url, &mut headers, "{}".into());
    Ok(sent.await?.status().as_u16())
}

#[tokio::test]
async fn a_held_origin_is_sent_no_byte_until_it_is_let_go() {
    let (url, heard) = recording().await;
    let hold = Arc::new(One {
        origin: Origin::of(&url).unwrap(),
        holding: AtomicBool::new(true),
        asked: Mutex::default(),
    });
    let http = client(hold.clone());

    let error = post(&http, &url).await.unwrap_err();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        matches!(&error, HttpError::Held(route) if &**route == "route"),
        "{error:?}"
    );
    assert_eq!(*heard.lock().unwrap(), 0, "bytes reached a held origin");

    hold.holding.store(false, Ordering::SeqCst);
    assert_eq!(post(&http, &url).await.unwrap(), 200);
    assert!(*heard.lock().unwrap() > 0);
}

#[tokio::test]
async fn a_clone_asks_the_same_hold_and_another_origin_is_not_held() {
    let (url, heard) = recording().await;
    let hold = Arc::new(One {
        origin: Origin::of("https://vendor.invalid").unwrap(),
        holding: AtomicBool::new(true),
        asked: Mutex::default(),
    });
    let http = client(hold.clone()).clone();

    assert_eq!(post(&http, &url).await.unwrap(), 200);
    assert!(*heard.lock().unwrap() > 0);
    let asked = hold.asked.lock().unwrap().clone();
    assert_eq!(asked, [Origin::of(&url).unwrap()]);
}

#[test]
fn one_place_spelled_several_ways_is_one_origin() {
    let one = Origin::of("https://api.kimi.com/coding/v1").unwrap();
    for spelled in [
        "https://API.Kimi.com/coding/v1",
        "https://api.kimi.com./coding/v1",
        "https://api.kimi.com:443/coding/v1",
        "HTTPS://api.kimi.com/other",
    ] {
        assert_eq!(Origin::of(spelled).unwrap(), one, "{spelled}");
    }
    for other in [
        "http://api.kimi.com/coding/v1",
        "https://api.kimi.com:8443/coding/v1",
        "https://kimi.com/coding/v1",
        "https://api.kimi.com.evil.example/coding/v1",
    ] {
        assert_ne!(Origin::of(other).unwrap(), one, "{other}");
    }
    assert_eq!(one.to_string(), "https://api.kimi.com");
    assert_eq!(
        Origin::of("http://127.0.0.1:8080/v1").unwrap().to_string(),
        "http://127.0.0.1:8080"
    );
    for nothing in ["ftp://api.kimi.com", "https://", "api.kimi.com/v1", ""] {
        assert!(Origin::of(nothing).is_none(), "{nothing}");
    }
}
