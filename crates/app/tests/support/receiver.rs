//! A webhook receiver: an HTTP listener that answers `200` and hands over each request.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

use lsf_app::admin::auth::signing::verify_webhook_hmac;
use serde_json::Value;

pub struct Receiver {
    pub url: String,
    secret: &'static str,
    requests: mpsc::Receiver<(HashMap<String, String>, Vec<u8>)>,
}

impl Receiver {
    /// Listens on a free port of `127.0.0.1`; deliveries are verified with `secret`.
    pub fn start(secret: &'static str) -> Receiver {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (sender, requests) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut headers = HashMap::new();
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = trimmed.split_once(':') {
                        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                    }
                }
                let length: usize = headers
                    .get("content-length")
                    .and_then(|length| length.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                if sender.send((headers, body)).is_err() {
                    break;
                }
            }
        });
        Receiver {
            url,
            secret,
            requests,
        }
    }

    /// The next request, as it came: its headers (names in lowercase) and its body.
    pub fn next_raw(&self) -> (HashMap<String, String>, Vec<u8>) {
        self.requests
            .recv_timeout(Duration::from_secs(10))
            .expect("a webhook was delivered")
    }

    /// The next delivery: its topic and its payload, its signature checked.
    pub fn next(&self) -> (String, Value) {
        let (headers, body) = self.next_raw();
        assert!(verify_webhook_hmac(
            self.secret,
            &body,
            &headers["x-shopify-hmac-sha256"]
        ));
        (
            headers["x-shopify-topic"].clone(),
            serde_json::from_slice(&body).unwrap(),
        )
    }

    /// The next `count` deliveries, by topic.
    pub fn take(&self, count: usize) -> Vec<(String, Value)> {
        (0..count).map(|_| self.next()).collect()
    }

    /// The next deliveries, which must be of these topics, in this order.
    pub fn expect(&self, topics: &[&str]) -> Vec<Value> {
        let delivered = self.take(topics.len());
        let got: Vec<&str> = delivered.iter().map(|(topic, _)| topic.as_str()).collect();
        assert_eq!(got, topics);
        delivered.into_iter().map(|(_, payload)| payload).collect()
    }

    /// Nothing more was delivered.
    pub fn quiet(&self) {
        assert!(
            self.requests
                .recv_timeout(Duration::from_millis(300))
                .is_err(),
            "a webhook more was delivered"
        );
    }
}
