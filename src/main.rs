#![feature(btree_cursors)]

mod apitree;
mod config;
mod exchange;
mod orderbook;
use crate::config::Config;
use actix::{Actor, StreamHandler};
use actix::{ActorContext, System};
use actix_web::{middleware, web, App, HttpRequest, HttpResponse, HttpServer};
use actix_web_actors::ws;
use actix_web_codegen::*;
use anyhow::{anyhow, Result};
use clap::Parser;
use exchange::Exchange;
use futures_util::{pin_mut, select, FutureExt, SinkExt, StreamExt};
use log::{error, info};
use orderbook::{AggregatedOrderbook, Orderbook};
use std::collections::HashMap;
use std::string::String;
use std::sync::Arc;
use std::vec::Vec;
use tokio::sync::broadcast::{self, error::RecvError, Sender};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::task::JoinHandle;
use tokio::task::JoinSet;

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

struct Session;

impl Session {
    pub fn new(_tx: broadcast::Sender<String>) -> Self {
        Self {}
    }
}

impl Actor for Session {
    type Context = ws::WebsocketContext<Self>;
    fn started(&mut self, ctx: &mut Self::Context) {}
}

type WsResult = Result<ws::Message, ws::ProtocolError>;

impl StreamHandler<WsResult> for Session {
    fn handle(&mut self, msg: WsResult, ctx: &mut Self::Context) {
        if msg.is_err() {
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
    pairs: Vec<String>,
    tx: UnboundedSender<(String, Orderbook)>,
) -> Result<()> {
    let mut client = Exchange::new(&exchange);
    info!("start executor: {}", exchange);
    client.connect(pairs).await?;
    info!("connect {}", exchange);
    // currently we only allow single subscription
    loop {
        match client.next().await {
            Ok(Some(orderbook)) => {
                tx.send((exchange.clone(), orderbook));
            }
            Ok(None) => {
                info!("shutdown {}", exchange);
                break;
            }
            Err(e) => {
                error!("{}", e);
            }
        }
    }
    Ok(())
}

async fn setup_marketdata(
    exchange_pairs: HashMap<String, Vec<String>>,
    tx: UnboundedSender<String>,
) {
    let (itx, mut irx) = unbounded_channel::<(String, Orderbook)>();
    let mut exchange_cache = HashMap::<String, Orderbook>::with_capacity(exchange_pairs.len());
    for (exchange, pairs) in exchange_pairs {
        info!("loading {}: {:?}", exchange, pairs);
        let ltx = itx.clone();
        std::thread::spawn(move || {
            let system = actix::System::new();
            let runtime = system.runtime();
            let result = runtime.block_on(executor(exchange.clone(), pairs.clone(), ltx));
            if let Err(e) = result {
                error!("exchange client spawn error: {}", e);
            }
        });
    }
    while let Some((exchange, orderbook)) = irx.recv().await {
        let mut agg = AggregatedOrderbook::new();
        exchange_cache.remove(&exchange);
        exchange_cache.insert(exchange.clone(), orderbook);
        for (_key, ob) in exchange_cache.iter() {
            agg.merge(ob);
        }
        match agg.finalize(10) {
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
    let (btx, mut brx) = broadcast::channel::<String>(20);
    let cbtx = btx.clone();
    tokio::spawn(async move {
        while let Some(item) = rx.recv().await {
            if let Err(e) = cbtx.send(item) {
                error!("{:?}", e);
            }
        }
    });
    tokio::spawn(async move {
        while let Ok(item) = brx.recv().await {
            info!("Summary {:?}", item);
        }
    });
    let server_port = config.inner.server_port;
    let handler1 = tokio::spawn(setup_marketdata(config.inner.exchange_pair_map, tx));

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
