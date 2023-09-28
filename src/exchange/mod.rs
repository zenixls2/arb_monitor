use crate::apitree;
use crate::config::ExchangeSetting;
use crate::orderbook::Orderbook;
use anyhow::{anyhow, Result};
use formatx::formatx;
use futures_util::stream::SplitStream;
use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info};
use std::vec::Vec;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::time::{sleep, Duration, Instant};
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};
use Message::*;

pub struct Exchange {
    name: String,
    level: u32,
    rx: Option<SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>>,
    utx: Option<UnboundedSender<Message>>,
    //cache: String,
    ws_api: bool,
    pairs: Vec<String>,
    wait_secs: u64,
    heartbeat_ts: Option<Instant>,
}

impl Exchange {
    pub fn new(name: &str) -> Exchange {
        Exchange {
            name: name.to_string(),
            level: 10,
            //cache: "".to_string(),
            ws_api: true,
            pairs: vec![],
            wait_secs: 0,
            heartbeat_ts: None,
            rx: None,
            utx: None,
        }
    }
    pub async fn connect(&mut self, pairs: Vec<ExchangeSetting>) -> Result<()> {
        self.pairs = pairs.iter().map(|e| e.pair.clone()).collect();
        let default_setup = pairs
            .get(0)
            .ok_or_else(|| anyhow!("should have at least one pair setting"))?;
        self.wait_secs = if default_setup.wait_secs > 0 {
            default_setup.wait_secs
        } else {
            1_u64
        };
        self.ws_api = default_setup.ws_api;
        if !self.ws_api {
            return Ok(());
        }
        info!("start connect, {}", self.name);
        let api = apitree::ws(&self.name)?;

        let mut url = api.endpoint.to_string();
        let render_url = api.render_url;
        if render_url {
            let p = self.pairs.join(",");

            info!("render Url: {}", p);
            url = formatx!(url, p).map_err(|e| anyhow!("{:?}", e))?;
        }
        info!("{}", url);

        let (ws_stream, result) = connect_async(url).await?;
        info!("{:?}", result);
        let (mut tx, rx) = ws_stream.split();
        self.rx = Some(rx);

        let (utx, mut urx) = unbounded_channel();

        self.utx = Some(utx);

        tokio::spawn(async move {
            while let Some(msg) = urx.recv().await {
                if let Err(e) = tx.send(msg).await {
                    error!("{}", e);
                }
            }
        });

        if !render_url {
            if let Some(utx) = self.utx.clone() {
                for pair in self.pairs.iter() {
                    let requests = api.subscribe_text(pair, 20)?;
                    info!("{:?}", requests);
                    for request in requests {
                        utx.send(Message::Text(request))?;
                    }
                }
            }
        }

        Ok(())
    }
    pub fn clear(&self) -> Result<()> {
        let api = apitree::ws(&self.name)?;
        (api.clear)();
        Ok(())
    }
    pub async fn next(&mut self) -> Result<Option<Orderbook>> {
        if !self.ws_api {
            let level = self.level;
            sleep(Duration::from_secs(self.wait_secs)).await;
            // only able to handle one pair
            if let Some(pair) = self.pairs.first() {
                return (apitree::rest(&self.name)?.orderbook)(pair.clone())
                    .await
                    .map(move |mut e| {
                        e.trim(level);
                        Some(e)
                    });
            } else {
                return Err(anyhow!("no pair assigned to the exchange"));
            }
        }
        let result = &mut self
            .rx
            .as_mut()
            .ok_or_else(|| anyhow!("Not connect yet. Please run connect first"))?;
        let api = apitree::ws(&self.name)?;
        let (wait_secs, msg) = api.heartbeat.unwrap_or((0, ""));
        if self.heartbeat_ts.is_none() && wait_secs > 0 {
            self.heartbeat_ts = Some(Instant::now());
        }
        loop {
            // sending heartbeats
            if let Some(now) = self.heartbeat_ts {
                if wait_secs < now.elapsed().as_secs() {
                    info!("send heartbeat to {}", self.name);
                    self.heartbeat_ts = Some(Instant::now());
                    if let Some(utx) = self.utx.as_ref() {
                        utx.send(Message::Binary(msg.into()))?;
                    }
                }
            }
            if let Some(result) = result.next().await {
                let raw = match result? {
                    Text(msg) => msg,
                    Binary(msg) => std::str::from_utf8(&msg)?.to_string(),
                    /*Continuation(item) => match item {
                        FirstText(b) | FirstBinary(b) | Continue(b) => {
                            self.cache += std::str::from_utf8(&b)?;
                            return Ok(None);
                        }
                        Last(b) => {
                            let output = self.cache.clone() + std::str::from_utf8(&b)?;
                            self.cache = "".to_string();
                            output
                        }
                    },*/
                    Ping(_) | Pong(_) => return Ok(None),
                    Close(_) => {
                        error!("stream gets closed: {}", self.name);
                        return Err(anyhow!("close {}", self.name));
                    }
                    Frame(_) => {
                        unreachable!();
                    }
                };

                debug!("{}: {}", self.name, raw);

                if let Some(mut e) = (apitree::ws(&self.name)?.parse)(raw)? {
                    e.trim(self.level);
                    return Ok(Some(e));
                }
                // skip none
            } else {
                return Ok(None);
            }
        }
    }
}
