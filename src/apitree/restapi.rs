use crate::orderbook::{Orderbook, Side};
use anyhow::{anyhow, Result};
use bigdecimal::BigDecimal;
use chrono::prelude::*;
use chrono::Duration;
use futures_util::future::{join3, Future};
use log::info;
use once_cell::sync::Lazy;
use serde::de;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::ops::Bound::{Excluded, Included};
use std::ops::Sub;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Mutex;

type OrderbookBoxedFuture = Box<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<Orderbook>>>>>;

pub struct Api {
    pub endpoint: &'static str,
    pub orderbook: OrderbookBoxedFuture,
}

pub struct Dummy {}

impl Dummy {
    pub fn get(&self, name: &str) -> Option<Api> {
        match name {
            "independentreserve" => Some(Api {
                endpoint: "https://api.independentreserve.com",
                orderbook: Box::new(|s| Box::pin(independentreserve_orderbook(s))),
            }),
            "btcmarkets" => Some(Api {
                endpoint: "https://api.btcmarkets.net",
                orderbook: Box::new(|s| Box::pin(btcmarkets_orderbook(s))),
            }),
            "coinspot" => Some(Api {
                endpoint: "https://www.coinspot.com.au",
                orderbook: Box::new(|s| Box::pin(coinspot_orderbook(s))),
            }),
            _ => None,
        }
    }
}

pub static REST_APIMAP: Dummy = Dummy {};

struct NaiveDateTimeVisitor;

impl<'de> de::Visitor<'de> for NaiveDateTimeVisitor {
    type Value = NaiveDateTime;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "a string represents chrono::NaiveDateTime")
    }

    fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S.%fZ") {
            Ok(t) => Ok(t),
            Err(_) => Err(de::Error::invalid_value(de::Unexpected::Str(s), &self)),
        }
    }
}

fn from_datestr<'de, D>(d: D) -> Result<NaiveDateTime, D::Error>
where
    D: de::Deserializer<'de>,
{
    d.deserialize_str(NaiveDateTimeVisitor)
}

#[derive(Deserialize, Debug)]
struct CoinspotTrade {
    //coin: String,
    //market: String,
    amount: f64,
    //total: f64,
    //rate: f64,
    #[serde(deserialize_with = "from_datestr")]
    solddate: NaiveDateTime,
}

static COINSPOT_TRADES: Lazy<Mutex<BTreeMap<NaiveDateTime, CoinspotTrade>>> =
    Lazy::new(|| Mutex::new(BTreeMap::new()));

async fn coinspot_orderbook(pair: String) -> Result<Orderbook> {
    let api = REST_APIMAP.get("coinspot").unwrap();
    let endpoint = api.endpoint;
    let mut ob = Orderbook::new("coinspot");

    let api = format!("{}/pubapi/v2/orders/open/{}", endpoint, pair);
    info!("calling {}...", api);

    #[derive(Deserialize, Debug)]
    struct Level {
        amount: f64, // amount that was bought
        rate: f64,   // latest buy/sell price for that coin
    }
    #[derive(Deserialize, Debug)]
    struct OpenMarketOrders {
        status: String,
        message: String,
        buyorders: Vec<Level>,
        sellorders: Vec<Level>,
    }
    let order_fut = async move {
        let response = reqwest::get(&api).await?;
        let orders: OpenMarketOrders = response.json().await?;
        Result::<_, anyhow::Error>::Ok(orders)
    };

    #[derive(Deserialize, Debug)]
    struct Price {
        last: String,
    }
    #[derive(Deserialize, Debug)]
    struct LatestPrice {
        status: String,
        #[serde(default)]
        message: String,
        prices: Price,
    }
    let [coin, market]: [&str; 2] = pair
        .split('/')
        .collect::<Vec<&str>>()
        .try_into()
        .map_err(|e| anyhow!("{:?}", e))?;
    let api = if market == "AUD" {
        format!("{}/pubapi/v2/latest/{}", endpoint, coin)
    } else {
        format!("{}/pubapi/v2/latest/{}", endpoint, pair)
    };
    info!("calling {}...", api);
    let price_fut = async move {
        let response = reqwest::get(&api).await?;
        let last_price: LatestPrice = response.json().await?;
        Result::<_, anyhow::Error>::Ok(last_price)
    };

    #[derive(Deserialize, Debug)]
    struct Trades {
        status: String,
        #[serde(default)]
        message: String,
        buyorders: Vec<CoinspotTrade>,
        // this is not used. Volume on buy == volume on sell
        // sellorders: Vec<CoinspotTrade>,
    }

    let api = format!("{}/pubapi/v2/orders/completed/{}", endpoint, pair);
    info!("calling {}...", api);
    let trade_fut = async move {
        let response = reqwest::get(&api).await?;
        let trades: Trades = response.json().await?;
        Result::<_, anyhow::Error>::Ok(trades)
    };
    let (orders_r, last_price_r, trade_r) = join3(order_fut, price_fut, trade_fut).await;
    let orders = orders_r?;
    let last_price = last_price_r?;
    let trades = trade_r?;

    if trades.status != "ok" {
        return Err(anyhow!("trade {} {}", trades.status, trades.message));
    }
    let mut total_amount = 0.;
    {
        let mut tmp = COINSPOT_TRADES.lock().unwrap();
        for trade in trades.buyorders {
            tmp.insert(trade.solddate, trade);
        }
        let now = Utc::now().naive_utc();
        let past = now.sub(Duration::hours(24));
        for (_, trade) in tmp.range((Excluded(&past), Included(&now))) {
            total_amount += trade.amount;
        }
        *tmp = tmp.split_off(&past);
    }
    ob.volume = BigDecimal::from_str(&format!("{}", total_amount))
        .map_err(|e| anyhow!("parse volume fail: {:?}", e))?;

    info!("{:?}", orders);
    if orders.status != "ok" {
        return Err(anyhow!("orders {}: {}", orders.status, orders.message));
    }
    for lvl in orders.buyorders {
        let price = BigDecimal::from_str(&format!("{}", lvl.rate))
            .map_err(|e| anyhow!("parse price fail: {}", e))?;
        let volume = BigDecimal::from_str(&format!("{}", lvl.amount))
            .map_err(|e| anyhow!("volume price fail: {}", e))?;
        ob.insert(Side::Bid, price, volume);
    }
    for lvl in orders.sellorders {
        let price = BigDecimal::from_str(&format!("{}", lvl.rate))
            .map_err(|e| anyhow!("parse price fail: {}", e))?;
        let volume = BigDecimal::from_str(&format!("{}", lvl.amount))
            .map_err(|e| anyhow!("parse volume fail: {}", e))?;
        ob.insert(Side::Ask, price, volume);
    }

    if last_price.status != "ok" {
        return Err(anyhow!(
            "last_price {}: {}",
            last_price.status,
            last_price.message
        ));
    }
    ob.last_price = BigDecimal::from_str(&last_price.prices.last)
        .map_err(|e| anyhow!("parse last price fail {}", e))?;

    Ok(ob)
}

async fn btcmarkets_orderbook(pair: String) -> Result<Orderbook> {
    let api = REST_APIMAP.get("btcmarkets").unwrap();
    #[derive(Deserialize, Debug)]
    struct OrderbookSnapshot {
        asks: Vec<[String; 2]>,
        bids: Vec<[String; 2]>,
    }
    #[derive(Deserialize, Debug)]
    struct MarketSummary {
        volume24h: String,
        #[serde(rename = "lastPrice")]
        last_price: String,
    }
    let endpoint = api.endpoint;
    let api = format!("{}/v3/markets/{}/orderbook", endpoint, pair);
    info!("calling {}...", api);
    let response = reqwest::get(&api).await.map_err(|e| anyhow!("{:?}", e))?;
    let shot: OrderbookSnapshot = response.json().await.map_err(|e| anyhow!("{}", e))?;

    let api = format!("{}/v3/markets/{}/ticker", endpoint, pair);
    info!("calling {}...", api);
    let response = reqwest::get(&api).await.map_err(|e| anyhow!("{:?}", e))?;
    let sum: MarketSummary = response.json().await.map_err(|e| anyhow!("{}", e))?;
    let mut ob = Orderbook::new("btcmarkets");

    for [p, v] in shot.bids {
        let price = BigDecimal::from_str(&p).map_err(|e| anyhow!("parse price fail: {:?}", e))?;
        let volume = BigDecimal::from_str(&v).map_err(|e| anyhow!("parse volume fail: {:?}", e))?;
        ob.insert(Side::Bid, price, volume);
    }
    for [p, v] in shot.asks {
        let price = BigDecimal::from_str(&p).map_err(|e| anyhow!("parse price fail: {:?}", e))?;
        let volume = BigDecimal::from_str(&v).map_err(|e| anyhow!("parse volume fail: {:?}", e))?;
        ob.insert(Side::Ask, price, volume);
    }
    ob.last_price = BigDecimal::from_str(&sum.last_price)
        .map_err(|e| anyhow!("parse last_price fail: {:?}", e))?;
    ob.volume =
        BigDecimal::from_str(&sum.volume24h).map_err(|e| anyhow!("parse volume fail: {:?}", e))?;
    Ok(ob)
}

async fn independentreserve_orderbook(pair: String) -> Result<Orderbook> {
    let api = REST_APIMAP.get("independentreserve").unwrap();
    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "PascalCase")]
    struct Level {
        price: f64,
        volume: f64,
    }
    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "PascalCase")]
    struct OrderbookSnapshot {
        buy_orders: Vec<Level>,
        sell_orders: Vec<Level>,
    }
    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "PascalCase")]
    struct MarketSummary {
        last_price: f64,
        day_volume_xbt: f64,
    }
    let args: Vec<&str> = pair.split('-').collect();
    if args.len() != 2 {
        return Err(anyhow!(
            "pair in wrong format: should be Xbt-aud but got {}",
            pair
        ));
    }
    let endpoint = api.endpoint;
    let api = format!(
        "{}/Public/GetOrderbook?primaryCurrencyCode={}&secondaryCurrencyCode={}",
        endpoint, args[0], args[1]
    );
    info!("calling {}...", api);
    let response = reqwest::get(&api).await.map_err(|e| anyhow!("{:?}", e))?;
    let shot: OrderbookSnapshot = response.json().await.map_err(|e| anyhow!("{}", e))?;

    let api = format!(
        "{}/Public/GetMarketSummary?primaryCurrencyCode={}&secondaryCurrencyCode={}",
        endpoint, args[0], args[1]
    );
    info!("calling {}...", api);
    let response = reqwest::get(&api).await.map_err(|e| anyhow!("{:?}", e))?;
    let sum: MarketSummary = response.json().await.map_err(|e| anyhow!("{}", e))?;
    let mut ob = Orderbook::new("independentreserve");
    for level in shot.buy_orders {
        let price = BigDecimal::from_str(&format!("{}", level.price))
            .map_err(|e| anyhow!("parse price fail: {:?}", e))?;
        let v = BigDecimal::from_str(&format!("{}", level.volume))
            .map_err(|e| anyhow!("parse volume fail: {:?}", e))?;
        ob.insert(Side::Bid, price, v);
    }
    for level in shot.sell_orders {
        let price = BigDecimal::from_str(&format!("{}", level.price))
            .map_err(|e| anyhow!("parse price fail: {:?}", e))?;
        let v = BigDecimal::from_str(&format!("{}", level.volume))
            .map_err(|e| anyhow!("parse volume fail: {:?}", e))?;
        ob.insert(Side::Ask, price, v);
    }
    ob.last_price = BigDecimal::from_str(&format!("{}", sum.last_price))
        .map_err(|e| anyhow!("parse last_price fail: {:?}", e))?;
    ob.volume = BigDecimal::from_str(&format!("{}", sum.day_volume_xbt))
        .map_err(|e| anyhow!("parse volume fail: {:?}", e))?;
    Ok(ob)
}
