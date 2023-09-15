use crate::orderbook::{Orderbook, Side};
use anyhow::{anyhow, Result};
use bigdecimal::BigDecimal;
use formatx::formatx;
use log::error;
use once_cell::sync::Lazy;
use phf::phf_map;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;

type ParseFunc = fn(String) -> Result<Option<Orderbook>>;
#[derive(Clone)]
pub struct Api {
    pub endpoint: &'static str,
    // (pair, level)
    pub subscribe_template: &'static [&'static str],
    // raw String as input
    pub parse: ParseFunc,
    // render url with data
    pub render_url: bool,
    // wait second, heartbeat message. None means no need to send heartbeat
    pub heartbeat: Option<(u64, &'static str)>,
    // cleanup function when error
    pub clear: fn() -> (),
}

impl Api {
    // utility to render the subscription text
    pub fn subscribe_text(&self, pair: &str, level: u32) -> Result<Vec<String>> {
        let mut result = vec![];
        for template in self.subscribe_template.iter() {
            result
                .push(formatx!(template.to_string(), pair, level).map_err(|e| anyhow!("{:?}", e))?);
        }
        Ok(result)
    }
}

fn binance_parser(raw: String) -> Result<Option<Orderbook>> {
    #[derive(Default, Deserialize, Debug)]
    #[serde(rename_all = "camelCase", default)]
    struct PartialBookDepth {
        last_update_id: u64,
        bids: Vec<[String; 2]>,
        asks: Vec<[String; 2]>,
        result: Value,
        id: u64,
    }
    // PartialBookDepth is the only subscription type
    // others should be categorized as error
    let result: PartialBookDepth = serde_json::from_str(&raw).map_err(|e| anyhow!("{:?}", e))?;
    // this is a subscription response
    if result.last_update_id == 0 && result.bids.is_empty() && result.asks.is_empty() {
        return Ok(None);
    }
    if result.result != Value::Null {
        return Err(anyhow!("result not empty"));
    }

    let mut ob = Orderbook::new("binance");
    for [price_str, quantity_str] in result.bids {
        let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
        let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
        ob.insert(Side::Bid, price, quantity);
    }
    for [price_str, quantity_str] in result.asks {
        let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
        let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
        ob.insert(Side::Ask, price, quantity);
    }
    Ok(Some(ob))
}

fn bitstamp_parser(raw: String) -> Result<Option<Orderbook>> {
    #[derive(Deserialize, Debug)]
    struct LiveDetailOrderbook {
        bids: Vec<[String; 2]>,
        asks: Vec<[String; 2]>,
        #[serde(rename = "timestamp")]
        _timestamp: String,
        #[serde(rename = "microtimestamp")]
        _microtimestamp: String,
    }
    #[derive(Deserialize, Debug)]
    struct WsEvent {
        data: Value,
        event: String,
        channel: String,
    }
    let result: WsEvent = serde_json::from_str(&raw).map_err(|e| anyhow!("{:?}", e))?;
    if result.event != "data" {
        // return an empty Orderbook. This might be a response or reconnect request
        // we'll ignore reconnection handling at this moment
        return Ok(None);
    }
    if !result.channel.starts_with("order_book_") {
        return Err(anyhow!("non-orderbook signal passed it"));
    }
    // LiveDetailOrderbook is the only subscription type
    // others should be categorized as error
    let result: LiveDetailOrderbook =
        serde_json::from_value(result.data).map_err(|e| anyhow!("{:?}", e))?;
    let mut ob = Orderbook::new("bitstamp");
    for [price_str, quantity_str] in result.bids {
        let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
        let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
        ob.insert(Side::Bid, price, quantity);
    }
    for [price_str, quantity_str] in result.asks {
        let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
        let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
        ob.insert(Side::Ask, price, quantity);
    }
    Ok(Some(ob))
}

static INDRESERVE: Lazy<Mutex<HashMap<String, Orderbook>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn indreserve_clear() {
    let mut tmp = INDRESERVE.lock().unwrap();
    tmp.clear();
}

fn indreserve_parser(raw: String) -> Result<Option<Orderbook>> {
    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "PascalCase")]
    struct Unit {
        price: f64,
        volume: f64,
    }
    #[derive(Deserialize, Debug)]
    struct Snapshot {
        #[serde(rename = "Bids")]
        bids: Vec<Unit>,
        #[serde(rename = "Offers")]
        asks: Vec<Unit>,
        #[serde(rename = "Crc32")]
        _crc32: u64,
    }
    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "PascalCase")]
    struct WsEvent {
        #[serde(default)]
        channel: String,
        #[serde(default)]
        data: Value,
        event: String,
    }
    let result: WsEvent = serde_json::from_str(&raw).map_err(|e| anyhow!("{:?}", e))?;
    if result.event == "Subscriptions" {
        let mut tmp = INDRESERVE.lock().unwrap();
        let result: Vec<String> =
            serde_json::from_value(result.data).map_err(|e| anyhow!("{:?}", e))?;
        for channel in result {
            tmp.insert(channel, Orderbook::new("independentreserve"));
        }
        return Ok(None);
    } else if result.event != "OrderBookSnapshot" && result.event != "OrderBookChange" {
        return Ok(None);
    }
    let mut tmp = INDRESERVE.lock().unwrap();
    if let Some(ob) = tmp.get_mut(&result.channel) {
        if result.event == "OrderBookSnapshot" {
            ob.ask.clear();
            ob.bid.clear();
        }
        let result: Snapshot =
            serde_json::from_value(result.data).map_err(|e| anyhow!("{:?}", e))?;
        for Unit { price, volume } in result.bids {
            let p = BigDecimal::from_str(&format!("{}", price))
                .map_err(|e| anyhow!("parse price fail: {} {:?}", price, e))?;
            let v = BigDecimal::from_str(&format!("{}", volume))
                .map_err(|e| anyhow!("parse volume fail: {} {:?}", volume, e))?;
            ob.insert(Side::Bid, p, v);
        }
        for Unit { price, volume } in result.asks {
            let p = BigDecimal::from_str(&format!("{}", price))
                .map_err(|e| anyhow!("parse price fail: {} {:?}", price, e))?;
            let v = BigDecimal::from_str(&format!("{}", volume))
                .map_err(|e| anyhow!("parse volume fail: {} {:?}", volume, e))?;
            ob.insert(Side::Ask, p, v);
        }
        Ok(Some(ob.clone()))
    } else {
        Err(anyhow!("orderbook not exist for {}", result.channel))
    }
}

static BTCMARKETS: Lazy<Mutex<HashMap<String, Orderbook>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn btcmarkets_clear() {
    let mut tmp = BTCMARKETS.lock().unwrap();
    tmp.clear();
    // 3 connections every 10 secs
    std::thread::sleep(std::time::Duration::from_secs(4));
}

fn btcmarkets_parser(raw: String) -> Result<Option<Orderbook>> {
    #[derive(Deserialize, Debug)]
    struct WsEvent {
        #[serde(default)]
        bids: Vec<[String; 2]>,
        #[serde(default)]
        asks: Vec<[String; 2]>,
        #[serde(default, rename = "lastPrice")]
        last_price: String,
        #[serde(default, rename = "volume24h")]
        volume: String,
        #[serde(rename = "messageType")]
        message_type: String,
        #[serde(default, rename = "marketId")]
        market_id: String,
    }
    let result: WsEvent = serde_json::from_str(&raw).map_err(|e| anyhow!("{:?}", e))?;
    let mut tmp = BTCMARKETS.lock().unwrap();
    let key = &result.market_id;
    let ob = if let Some(ob) = tmp.get_mut(key) {
        ob
    } else {
        tmp.insert(key.clone(), Orderbook::new("btcmarkets"));
        tmp.get_mut(key).unwrap()
    };
    if result.message_type == "orderbook" {
        ob.ask.clear();
        ob.bid.clear();
        for [price_str, quantity_str] in result.bids {
            let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
            let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
            ob.insert(Side::Bid, price, quantity);
        }
        for [price_str, quantity_str] in result.asks {
            let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
            let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
            ob.insert(Side::Ask, price, quantity);
        }
        return Ok(Some(ob.clone()));
    } else if result.message_type == "tick" {
        ob.last_price = BigDecimal::from_str(&result.last_price).map_err(|e| anyhow!("{:?}", e))?;
        ob.volume = BigDecimal::from_str(&result.volume).map_err(|e| anyhow!("{:?}", e))?;
        return Ok(Some(ob.clone()));
    } else {
        error!("btcmarket error dump: {}", raw);
    }
    Ok(None)
}

static COINJAR: Lazy<Mutex<HashMap<String, Orderbook>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn coinjar_clear() {
    let mut tmp = COINJAR.lock().unwrap();
    tmp.clear();
}

fn coinjar_parser(raw: String) -> Result<Option<Orderbook>> {
    #[derive(Deserialize, Debug)]
    struct WsEvent {
        event: String,
        payload: Value,
        topic: String,
    }
    let result: WsEvent = serde_json::from_str(&raw).map_err(|e| anyhow!("{:?}", e))?;
    if result.event != "init" && result.event != "update" {
        return Ok(None);
    }

    let mut tmp = COINJAR.lock().unwrap();
    if result.topic.starts_with("ticker") {
        let key = result.topic.replace("ticker:", "");
        let ob = if let Some(ob) = tmp.get_mut(&key) {
            ob
        } else {
            tmp.insert(key.clone(), Orderbook::new("coinjar"));
            tmp.get_mut(&key).unwrap()
        };
        #[derive(Deserialize, Debug)]
        struct Payload {
            #[serde(default)]
            volume: String,
            #[serde(default)]
            last: String,
        }
        let result: Payload =
            serde_json::from_value(result.payload.clone()).map_err(|e| anyhow!("{:?}", e))?;
        ob.volume = BigDecimal::from_str(&result.volume).map_err(|e| anyhow!("{:?}", e))?;
        ob.last_price = BigDecimal::from_str(&result.last).map_err(|e| anyhow!("{:?}", e))?;
        return Ok(Some(ob.clone()));
    } else if result.topic.starts_with("book") {
        let key = result.topic.replace("book:", "");
        let ob = if let Some(ob) = tmp.get_mut(&key) {
            ob
        } else {
            tmp.insert(key.clone(), Orderbook::new("coinjar"));
            tmp.get_mut(&key).unwrap()
        };
        if result.event == "init" {
            ob.ask.clear();
            ob.bid.clear();
        }
        #[derive(Deserialize, Debug)]
        struct Payload {
            #[serde(default)]
            bids: Vec<[String; 2]>,
            #[serde(default)]
            asks: Vec<[String; 2]>,
        }
        let result: Payload =
            serde_json::from_value(result.payload.clone()).map_err(|e| anyhow!("{:?}", e))?;
        for [price_str, quantity_str] in result.bids {
            let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
            let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
            ob.insert(Side::Bid, price, quantity);
        }
        for [price_str, quantity_str] in result.asks {
            let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
            let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
            ob.insert(Side::Ask, price, quantity);
        }
        return Ok(Some(ob.clone()));
    }
    Ok(None)
}

static KRAKEN: Lazy<Mutex<HashMap<String, Orderbook>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn kraken_clear() {
    let mut tmp = KRAKEN.lock().unwrap();
    tmp.clear();
}

fn kraken_parser(raw: String) -> Result<Option<Orderbook>> {
    if raw.as_bytes()[0] as char == '{' {
        return Ok(None);
    }
    let result: Vec<Value> = serde_json::from_str(&raw).map_err(|e| anyhow!("{:?}", e))?;
    let channel_name: String =
        serde_json::from_value(result[2].clone()).map_err(|e| anyhow!("{:?}", e))?;
    let pair: String = serde_json::from_value(result[3].clone()).map_err(|e| anyhow!("{:?}", e))?;
    let key = &pair;
    if channel_name.starts_with("book") {
        #[derive(Deserialize, Debug)]
        struct Data {
            #[serde(default)]
            r#as: Vec<[String; 3]>,
            #[serde(default)]
            bs: Vec<[String; 3]>,
            #[serde(default)]
            a: Vec<Vec<String>>,
            #[serde(default)]
            b: Vec<Vec<String>>,
        }
        // channel_id: u64
        // data: object
        // - as: Vec<[String; 3]>
        // - bs: Vec<[String; 3]>
        // channel_name: String
        // pair: String
        let data: Data =
            serde_json::from_value(result[1].clone()).map_err(|e| anyhow!("{:?}", e))?;
        let mut tmp = KRAKEN.lock().unwrap();
        let ob = if let Some(ob) = tmp.get_mut(key) {
            ob
        } else {
            tmp.insert(key.clone(), Orderbook::new("kraken"));
            tmp.get_mut(key).unwrap()
        };
        if data.bs.len() > 0 || data.r#as.len() > 0 {
            ob.bid.clear();
            ob.ask.clear();
        }
        for [price_str, quantity_str, _timestamp] in data.bs {
            let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
            let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
            ob.insert(Side::Bid, price, quantity);
        }
        for v in data.b {
            let price_str: &str = &v[0];
            let quantity_str: &str = &v[1];
            let price = BigDecimal::from_str(price_str).map_err(|e| anyhow!("{:?}", e))?;
            let quantity = BigDecimal::from_str(quantity_str).map_err(|e| anyhow!("{:?}", e))?;
            ob.insert(Side::Bid, price, quantity);
        }
        for [price_str, quantity_str, _timestamp] in data.r#as {
            let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
            let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
            ob.insert(Side::Ask, price, quantity);
        }
        for v in data.a {
            let price_str: &str = &v[0];
            let quantity_str: &str = &v[1];
            let price = BigDecimal::from_str(price_str).map_err(|e| anyhow!("{:?}", e))?;
            let quantity = BigDecimal::from_str(quantity_str).map_err(|e| anyhow!("{:?}", e))?;
            ob.insert(Side::Ask, price, quantity);
        }
        return Ok(Some(ob.clone()));
    } else if channel_name == "ticker".to_string() {
        // data:
        // - a: best ask [3]
        // - b: best bid [3]
        // - c: close [2]
        // - v: volume [2] (today, last24hr)
        #[derive(Deserialize, Debug)]
        struct Data {
            #[serde(default)]
            c: [String; 2],
            #[serde(default)]
            v: [String; 2],
        }
        let data: Data =
            serde_json::from_value(result[1].clone()).map_err(|e| anyhow!("{:?}", e))?;
        let mut tmp = KRAKEN.lock().unwrap();
        let ob = if let Some(ob) = tmp.get_mut(key) {
            ob
        } else {
            tmp.insert(key.clone(), Orderbook::new("kraken"));
            tmp.get_mut(key).unwrap()
        };
        ob.volume = BigDecimal::from_str(&data.v[1]).map_err(|e| anyhow!("{:?}", e))?;
        ob.last_price = BigDecimal::from_str(&data.c[0]).map_err(|e| anyhow!("{:?}", e))?;
        return Ok(Some(ob.clone()));
    }
    Ok(None)
}

// The API Map compile-time static map that handles depth orderbook subscription and parsing
pub static WS_APIMAP: phf::Map<&'static str, Api> = phf_map! {
    "binance" => Api {
        endpoint: "wss://stream.binance.com:9443/ws",
        subscribe_template: &[r#"{{"id": 1, "method": "SUBSCRIBE", "params": ["{}@depth{}@100ms"]}}"#],
        parse: (binance_parser as ParseFunc),
        render_url: false,
        heartbeat: None,
        clear: || {},
    },
    "binance_futures" => Api {
        endpoint: "wss://fstream.binance.com:9443/ws",
        subscribe_template: &[r#"{{"id":1, "method":"SUBSCRIBE", "params": ["{}@depth{}@100ms"]}}"#],
        parse: (binance_parser as ParseFunc),
        render_url: false,
        heartbeat: None,
        clear: || {},
    },
    "bitstamp" => Api {
        endpoint: "wss://ws.bitstamp.net",
        subscribe_template: &[r#"{{"event":"bts:subscribe","data":{{"channel":"order_book_{}"}}}}"#],
        parse: (bitstamp_parser as ParseFunc),
        render_url: false,
        heartbeat: None,
        clear: || {},
    },
    "independentreserve" => Api {
        endpoint: "wss://websockets.independentreserve.com/orderbook/20?subscribe={}",
        subscribe_template: &[r#"{{"Event": "Subscribe", "Data": ["{}"]}}"#],
        parse: (indreserve_parser as ParseFunc),
        render_url: true,
        heartbeat: None,
        clear: indreserve_clear,
    },
    "btcmarkets" => Api {
        endpoint: "wss://socket.btcmarkets.net/v2",
        subscribe_template: &[r#"{{"marketIds": ["{}"], "channels": ["orderbook", "tick"], "messageType": "subscribe"}}"#],
        parse: (btcmarkets_parser as ParseFunc),
        render_url: false,
        heartbeat: None,
        clear: btcmarkets_clear,
    },
    "coinjar" => Api {
        endpoint: "wss://feed.exchange.coinjar.com/socket/websocket",
        subscribe_template: &[
            r#"{{"topic": "book:{}", "event": "phx_join", "payload": {{}}, "ref": 0}}"#,
            r#"{{"topic": "ticker:{}", "event": "phx_join", "payload": {{}}, "ref": 0}}"#,
        ],
        parse: (coinjar_parser as ParseFunc),
        render_url: false,
        // this will disconnect the websocket
        //heartbeat: Some((10, r#"{{"topic": "phoenix", "event": "heartbeat", "payload": {{}}, "ref": 0}}"#)),
        heartbeat: None,
        clear: coinjar_clear,
    },
    "kraken" => Api {
        endpoint: "wss://ws.kraken.com",
        subscribe_template: &[
            r#"{{"event":"subscribe","pair":["{}"], "subscription": {{"name":"book","depth":25}}}}"#,
            r#"{{"event":"subscribe","pair":["{}"], "subscription": {{"name":"ticker"}}}}"#],
        parse: (kraken_parser as ParseFunc),
        render_url: false,
        heartbeat: None,
        clear: kraken_clear,
    }
};

#[cfg(test)]
mod tests {
    use bigdecimal::BigDecimal;
    use std::str::FromStr;
    #[test]
    fn test_subscribe_text() {
        let rendered = super::WS_APIMAP
            .get("binance")
            .unwrap()
            .subscribe_text("BTCUSDT", 20)
            .unwrap();
        assert_eq!(
            rendered,
            vec![r#"{"id": 1, "method": "SUBSCRIBE", "params": ["BTCUSDT@depth20@100ms"]}"#]
        );
    }
    #[test]
    fn test_binance_parse() {
        // subscription response, return empty Orderbook
        let out = (super::WS_APIMAP.get("binance").unwrap().parse)(
            r#"{"id": 1, "result": null}"#.to_string(),
        )
        .unwrap();
        assert_eq!(out, None);

        // normal event
        let out = (super::WS_APIMAP.get("binance").unwrap().parse)(
            r#"{"lastUpdateId": 160, "bids":[["0.01", "0.2"]], "asks": []}"#.to_string(),
        )
        .unwrap();
        let mut ob = super::Orderbook::new("binance");
        ob.insert(
            super::Side::Bid,
            BigDecimal::from_str("0.01").unwrap(),
            BigDecimal::from_str("0.2").unwrap(),
        );
        if let Some(o) = out.as_ref() {
            ob.timestamp = o.timestamp;
        }
        assert_eq!(out, Some(ob));
    }
    #[test]
    fn test_bitstamp_parse() {
        // subscription response
        let out = (super::WS_APIMAP.get("bitstamp").unwrap().parse)(
            r#"{"event": "bts:subscription_succeeded", "channel": "order_book_btcusd", "data": {}}"#
                .to_string(),
        )
        .unwrap();
        assert_eq!(out, None);

        // normal event
        let out = (super::WS_APIMAP.get("bitstamp").unwrap().parse)(
            r#"{"data":{
                "timestamp":"1691595437",
                "microtimestamp":"1691595437334962",
                "bids":[],
                "asks":[["29737","0.67548438"],["29738","0.67255217"]]
            },"channel":"order_book_btcusd","event":"data"}"#
                .to_string(),
        )
        .unwrap();
        let mut ob = super::Orderbook::new("bitstamp");
        ob.insert(
            super::Side::Ask,
            BigDecimal::from_str("29737").unwrap(),
            BigDecimal::from_str("0.67548438").unwrap(),
        );
        ob.insert(
            super::Side::Ask,
            BigDecimal::from_str("29738").unwrap(),
            BigDecimal::from_str("0.67255217").unwrap(),
        );
        assert_eq!(out, Some(ob));
    }
    #[test]
    fn test_indreserve_parse() {
        // subscription response
        (super::WS_APIMAP.get("independentreserve").unwrap().parse)(
            r#"{"Data": ["orderbook/5/btc/aud"], "Event": "Subscriptions", "Time": 1660895883834}"#
                .to_string(),
        )
        .unwrap();
        let out = (super::WS_APIMAP.get("independentreserve").unwrap().parse)(
            r#"{"Channel": "orderbook/5/btc/aud","Data": {
                "Bids": [{
                    "Price": 31802.46,"Volume": 0.25
                },{
                    "Price": 31802.45,"Volume": 0.32464684
                }],
                "Offers": [{
                    "Price": 31844.99,"Volume": 0.30740328
                },{
                    "Price": 31845,"Volume": 1.5
                }],
                "Crc32": 2893776693
              },
              "Time": 1660895883834,"Event": "OrderBookSnapshot"
            }"#
            .to_string(),
        )
        .unwrap();
        let mut ob = super::Orderbook::new("independentreserve");
        ob.insert(
            super::Side::Bid,
            BigDecimal::from_str("31802.46").unwrap(),
            BigDecimal::from_str("0.25").unwrap(),
        );
        ob.insert(
            super::Side::Bid,
            BigDecimal::from_str("31802.45").unwrap(),
            BigDecimal::from_str("0.32464684").unwrap(),
        );
        ob.insert(
            super::Side::Ask,
            BigDecimal::from_str("31844.99").unwrap(),
            BigDecimal::from_str("0.30740328").unwrap(),
        );
        ob.insert(
            super::Side::Ask,
            BigDecimal::from_str("31845").unwrap(),
            BigDecimal::from_str("1.5").unwrap(),
        );
        assert_eq!(out, Some(ob));
    }
}
