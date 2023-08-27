#![feature(btree_cursors)]

mod apitree;
mod config;
mod exchange;
mod orderbook;
use crate::config::Config;
use actix::{Actor, StreamHandler};
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer};
use actix_web_actors::ws;
use anyhow::{anyhow, Result};
use clap::Parser;
use exchange::Exchange;
use futures_util::{pin_mut, select, FutureExt, SinkExt, StreamExt};
use log::{error, info};
use orderbook::{AggregatedOrderbook, Orderbook};
use std::string::String;
use std::vec::Vec;

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

async fn setup_marketdata(exchange_pairs: Vec<(&str, &str)>) -> Result<()> {
    for (exchange, pair) in exchange_pairs {
        info!("loading {}: {}", exchange, pair);
        let mut client = Exchange::new(exchange);
        client.connect().await?;
        client.subscribe(pair).await?;
    }
    Ok(())
}

struct Session;

impl Actor for Session {
    type Context = ws::WebsocketContext<Self>;
}

type WsResult = Result<ws::Message, ws::ProtocolError>;

impl StreamHandler<WsResult> for Session {
    fn handle(&mut self, msg: WsResult, ctx: &mut Self::Context) {
        match msg {
            Ok(ws::Message::Ping(p)) => ctx.pong(&p),
            Ok(ws::Message::Text(text)) => ctx.text(text),
            Ok(ws::Message::Binary(bin)) => ctx.binary(bin),
            _ => (),
        }
    }
}

async fn listener(
    req: HttpRequest,
    stream: web::Payload,
) -> Result<HttpResponse, actix_web::Error> {
    let result = ws::start(Session {}, &req, stream);
    info!("{:?}", result);
    result
}

#[actix::main]
async fn main() -> Result<()> {
    let mut config = Config::parse();
    println!("loading from {}", config.config_path);
    config.load()?;

    setup_logger(config.inner.log_path, config.inner.log_level)?;

    let bind_addr = config
        .inner
        .bind_addr
        .unwrap_or_else(|| "0.0.0.0".to_string());

    HttpServer::new(|| App::new().route("/ws/", web::get().to(listener)))
        .bind((bind_addr, config.inner.server_port))?
        .run()
        .await
        .map_err(|e| anyhow!("{:?}", e))
}
