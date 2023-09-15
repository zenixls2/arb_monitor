#![feature(btree_cursors, io_error_other)]

mod apitree;
mod config;
mod exchange;
mod orderbook;
use crate::config::Config;
use actix::{Actor, ActorContext, AsyncContext, StreamHandler};
use actix_web::{middleware, web, App, HttpRequest, HttpResponse, HttpServer};
use actix_web_actors::ws;
use actix_web_codegen::*;
use anyhow::{anyhow, Result};
use clap::Parser;
use config::ExchangeSetting;
use exchange::Exchange;
use futures_util::StreamExt;
use log::{error, info};
use once_cell::sync::Lazy;
use orderbook::{AggregatedOrderbook, Orderbook};
use std::collections::HashMap;
use std::string::String;
use std::sync::Mutex;
use std::vec::Vec;
use tokio::sync::broadcast;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_stream::wrappers::BroadcastStream;

fn setup_logger(
    log_file: Option<String>,
    log_level: config::LogLevel,
) -> Result<(), fern::InitError> {
    let tmp = fern::Dispatch::new()
        .format(|out, message, _record| out.finish(format_args!("{}", message)))
        .level(log_level.to_level_filter())
        .chain(std::io::stdout());
    if let Some(path) = log_file {
        tmp.chain(fern::log_file(path)?).apply()?;
    } else {
        tmp.apply()?;
    }
    Ok(())
}

struct Session {
    tx: broadcast::Sender<String>,
}

impl Session {
    pub fn new(tx: broadcast::Sender<String>) -> Self {
        Self { tx }
    }
}

static CACHE: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

impl Actor for Session {
    type Context = ws::WebsocketContext<Self>;
    fn started(&mut self, ctx: &mut Self::Context) {
        let rx = BroadcastStream::new(self.tx.subscribe()).map(|e| {
            e.map(|s| ws::Message::Text(s.into()))
                .map_err(|e| ws::ProtocolError::Io(std::io::Error::other(e)))
        });
        // send previous record on connect
        let tmp = CACHE.lock().unwrap();
        if tmp.is_some() {
            ctx.text(tmp.clone().unwrap());
        }
        ctx.add_stream(rx);
    }
}

type WsResult = Result<ws::Message, ws::ProtocolError>;

impl StreamHandler<WsResult> for Session {
    fn handle(&mut self, msg: WsResult, ctx: &mut Self::Context) {
        if msg.is_err() {
            error!("{:?}", msg);
            ctx.stop();
            return;
        }

        match msg.unwrap() {
            ws::Message::Ping(p) => {
                info!("ping {:?}", p);
            }
            ws::Message::Text(text) => {
                info!("recv {}", text);
                ctx.text(text);
            }
            ws::Message::Pong(_) => {
                info!("pong");
            }
            ws::Message::Binary(bin) => {
                info!("recv bin {:?}", bin);
                ctx.binary(bin);
            }
            _ => (),
        }
    }
    fn finished(&mut self, _ctx: &mut Self::Context) {
        info!("finished");
    }
}

#[get("/ws")]
async fn websocket(
    req: HttpRequest,
    stream: web::Payload,
) -> Result<HttpResponse, actix_web::Error> {
    let tx = req.app_data::<broadcast::Sender<String>>().unwrap();
    let tx = tx.clone();
    ws::start(Session::new(tx), &req, stream)
}

async fn executor(
    exchange: String,
    pairs: Vec<ExchangeSetting>,
    tx: UnboundedSender<(String, Orderbook)>,
) -> Result<()> {
    let mut client = Exchange::new(&exchange);
    info!("start executor: {}", exchange);
    client.connect(pairs.clone()).await?;
    info!("connect {}", exchange);
    // currently we only allow single subscription
    loop {
        match client.next().await {
            Ok(Some(orderbook)) => {
                tx.send((exchange.clone(), orderbook))?;
                continue;
            }
            Ok(None) => {
                error!("shutdown {}", exchange);
            }
            Err(e) => {
                error!("{}, reconnect...", e);
            }
        }
        if let Err(e) = client.clear() {
            error!("{}, clear error", e);
        }
        client = Exchange::new(&exchange);
        if let Err(e) = client.connect(pairs.clone()).await {
            error!("{}, connect error {}", e, exchange);
        }
        error!("connect {}", exchange);
    }
}

async fn setup_marketdata(
    exchange_pairs: HashMap<String, Vec<ExchangeSetting>>,
    tx: UnboundedSender<String>,
) {
    let (itx, mut irx) = unbounded_channel::<(String, Orderbook)>();
    let mut exchange_cache = HashMap::<String, Orderbook>::with_capacity(exchange_pairs.len());
    let mut threads = vec![];
    for (exchange, settings) in exchange_pairs {
        info!("loading {}: {:?}", exchange, settings);
        let ltx = itx.clone();
        threads.push(std::thread::spawn(move || {
            let system = actix::System::new();
            let runtime = system.runtime();
            let result = runtime.block_on(executor(exchange.clone(), settings.clone(), ltx));
            if let Err(e) = result {
                error!("exchange client spawn error: {}", e);
            }
        }));
    }
    while let Some((exchange, orderbook)) = irx.recv().await {
        let mut agg = AggregatedOrderbook::new();
        exchange_cache.remove(&exchange);
        exchange_cache.insert(exchange.clone(), orderbook);
        for (_key, ob) in exchange_cache.iter() {
            agg.merge(ob);
        }
        match agg.finalize() {
            Ok(result) => {
                let summary = serde_json::to_string(&result).unwrap();
                if let Err(e) = tx.send(summary) {
                    error!("{:?}", e);
                }
            }
            Err(e) => {
                error!("{:?}", e);
            }
        }
    }
    threads.clear();
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut config = Config::parse();
    println!("loading from {}", config.config_path);
    config.load()?;

    setup_logger(config.inner.log_path, config.inner.log_level)?;

    let bind_addr = config
        .inner
        .bind_addr
        .unwrap_or_else(|| "0.0.0.0".to_string());

    let (tx, mut rx) = unbounded_channel::<String>();
    let (btx, mut brx) = broadcast::channel::<String>(100);
    let cbtx = btx.clone();
    // forward message from unbounded channel to broadcast channel
    tokio::spawn(async move {
        while let Some(item) = rx.recv().await {
            if let Err(e) = cbtx.send(item) {
                error!("{:?}", e);
            }
        }
    });

    // default consumer
    tokio::spawn(async move {
        while let Ok(item) = brx.recv().await {
            let mut tmp = CACHE.lock().unwrap();
            info!("Summary {}", tmp.insert(item));
        }
    });

    // subscribe to multiple exchanges
    // TODO: rewrite using tungstenite
    let server_port = config.inner.server_port;
    tokio::spawn(setup_marketdata(config.inner.exchange_pair_map, tx));

    // websocket server for broadcasting states
    HttpServer::new(move || {
        App::new()
            .app_data(btx.clone())
            .service(websocket)
            .wrap(middleware::Logger::default())
    })
    .bind((bind_addr, server_port))
    .map_err(|e| anyhow!("{:?}", e))?
    .run()
    .await
    .map_err(|e| anyhow!("{:?}", e))?;

    Ok(())
}
