use anyhow::Result;
use bigdecimal::{BigDecimal, Zero};
use log::error;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::time::SystemTime;

#[derive(Clone, Copy)]
pub enum Side {
    Bid,
    Ask,
}

fn get_unixtime() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

#[derive(Debug, PartialEq, Clone)]
pub struct Orderbook {
    pub(crate) name: String,
    pub(crate) timestamp: u128,
    pub(crate) volume: BigDecimal,
    pub(crate) last_price: BigDecimal,
    pub(crate) bid: BTreeMap<BigDecimal, BigDecimal>,
    pub(crate) ask: BTreeMap<BigDecimal, BigDecimal>,
}

impl Orderbook {
    pub fn insert(&mut self, side: Side, price: BigDecimal, volume: BigDecimal) {
        match side {
            Side::Bid => {
                self.bid.remove(&price);
                if !volume.is_zero() {
                    self.bid.insert(price, volume);
                }
            }
            Side::Ask => {
                self.ask.remove(&price);
                if !volume.is_zero() {
                    self.ask.insert(price, volume);
                }
            }
        };
        // some exchange doesn't provide timestamp in their websocket events.
        // use local timestamp to have the same basis
        self.timestamp = get_unixtime();

        let best_bid = self.bid.last_key_value().map(|(p, _)| p);
        let best_ask = self.ask.first_key_value().map(|(p, _)| p);
        if let (Some(bb), Some(ba)) = (best_bid, best_ask) {
            if bb > ba {
                error!("{} crossed!, {}, {}", self.name, bb, ba);
            }
        }
    }
    pub fn new(name: &str) -> Orderbook {
        Orderbook {
            name: name.to_string(),
            bid: BTreeMap::new(),
            ask: BTreeMap::new(),
            timestamp: get_unixtime(),
            last_price: BigDecimal::zero(),
            volume: BigDecimal::zero(),
        }
    }
    // used to trim bid/ask to level numbers of price bars
    pub fn trim(&mut self, level: u32) {
        let l = self.bid.len();
        for _ in (level as usize)..l {
            self.bid.pop_first();
        }
        let l = self.ask.len();
        for _ in (level as usize)..l {
            self.ask.pop_last();
        }
    }
}

// AggregatedOrderbook works like this:
// new() -> merge(ob1) -> merge(ob2) -> ... -> merge(obN) -> finalize(max_level)
// max_level here is used to limit the depth of orderbook to reach in this call
#[derive(Debug)]
pub struct AggregatedOrderbook {
    pub spread: f64,
    pub bid: BTreeMap<BigDecimal, Vec<(String, BigDecimal)>>,
    pub ask: BTreeMap<BigDecimal, Vec<(String, BigDecimal)>>,
    pub timestamp: HashMap<String, u128>,
    pub volume: HashMap<String, BigDecimal>,
    pub last_price: HashMap<String, BigDecimal>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Level {
    exchange: String,
    price: String,
    amount: String,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub spread: String,
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
    pub timestamp: HashMap<String, String>,
    pub volume: HashMap<String, String>,
    pub last_price: HashMap<String, String>,
}

impl AggregatedOrderbook {
    // merge the content from one orderbook
    pub fn merge(&mut self, orderbook: &Orderbook) {
        let name = &orderbook.name;
        let mut counter = 0;
        for (price, volume) in orderbook.bid.iter() {
            counter += 1;
            self.bid
                .entry(price.clone())
                .and_modify(|e| e.push((name.clone(), volume.clone())))
                .or_insert_with(|| vec![(name.clone(), volume.clone())]);

            if counter == 10 {
                break;
            }
        }
        let mut counter = 0;
        for (price, volume) in orderbook.ask.iter() {
            counter += 1;
            self.ask
                .entry(price.clone())
                .and_modify(|e| e.push((name.clone(), volume.clone())))
                .or_insert_with(|| vec![(name.clone(), volume.clone())]);
            if counter == 10 {
                break;
            }
        }
        self.spread = 0.0;
        self.timestamp.remove(name);
        self.timestamp.insert(name.clone(), orderbook.timestamp);
        self.volume.remove(name);
        self.volume.insert(name.clone(), orderbook.volume.clone());
        self.last_price.remove(name);
        self.last_price
            .insert(name.clone(), orderbook.last_price.clone());
    }
    pub fn new() -> AggregatedOrderbook {
        AggregatedOrderbook {
            spread: std::f64::NAN,
            bid: BTreeMap::new(),
            ask: BTreeMap::new(),
            timestamp: HashMap::new(),
            last_price: HashMap::new(),
            volume: HashMap::new(),
        }
    }
    // calculate the spread, output the stored price and volume data to Summary
    pub fn finalize(&mut self) -> Result<Summary> {
        let mut cursor = self.bid.upper_bound(Bound::Unbounded);
        let last_price = self
            .last_price
            .iter()
            .map(|(e, t)| (e.clone(), t.to_string()))
            .collect();
        let volume = self
            .volume
            .iter()
            .map(|(e, t)| (e.clone(), t.to_string()))
            .collect();
        let timestamp = self
            .timestamp
            .iter()
            .map(|(e, t)| (e.clone(), t.to_string()))
            .collect();
        let mut bids = vec![];
        while let Some((price, v)) = cursor.key_value() {
            for (exchange, volume) in v.iter() {
                bids.push(Level {
                    exchange: exchange.clone(),
                    price: price.to_string(),
                    amount: volume.to_string(),
                });
            }
            // notice move_prev is to move to the previous element in tree,
            // not the order of upper bound or lower bound.
            if cursor.peek_prev().is_some() {
                cursor.move_prev();
            } else {
                break;
            }
        }
        let mut cursor = self.ask.lower_bound(Bound::Unbounded);
        let mut asks = vec![];
        while let Some((price, v)) = cursor.key_value() {
            for (exchange, volume) in v.iter() {
                asks.push(Level {
                    exchange: exchange.clone(),
                    price: price.to_string(),
                    amount: volume.to_string(),
                });
            }
            if cursor.peek_next().is_some() {
                cursor.move_next();
            } else {
                break;
            }
        }
        let best_bid = self.bid.last_key_value().map(|(p, _)| p);
        let best_ask = self.ask.first_key_value().map(|(p, _)| p);
        let spread = match (best_bid, best_ask) {
            (Some(v), Some(w)) => (w - v).to_string(),
            _ => "0".to_string(),
        };
        Ok(Summary {
            spread,
            bids,
            asks,
            timestamp,
            last_price,
            volume,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_orderbook_trim() {
        let default_quantity: BigDecimal = BigDecimal::from_str("10").unwrap();
        let mut ob = Orderbook::new("");
        ob.insert(
            Side::Ask,
            BigDecimal::from_str("1").unwrap(),
            default_quantity.clone(),
        );
        ob.insert(
            Side::Ask,
            BigDecimal::from_str("2").unwrap(),
            default_quantity.clone(),
        );
        ob.trim(1);
        assert_eq!(ob.bid.len(), 0);
        assert_eq!(ob.ask.len(), 1);
        let one = BigDecimal::from_str("1").unwrap();
        assert_eq!(ob.ask.first_key_value(), Some((&one, &default_quantity)));
    }
    #[test]
    fn test_agg_merge() {
        let default_quantity: BigDecimal = BigDecimal::from_str("10").unwrap();
        let mut ob1 = Orderbook::new("A");
        ob1.insert(
            Side::Ask,
            BigDecimal::from_str("1").unwrap(),
            default_quantity.clone(),
        );
        ob1.insert(
            Side::Ask,
            BigDecimal::from_str("2").unwrap(),
            default_quantity.clone(),
        );
        let mut ob2 = Orderbook::new("B");
        ob2.insert(
            Side::Ask,
            BigDecimal::from_str("1").unwrap(),
            default_quantity.clone(),
        );
        ob2.insert(
            Side::Ask,
            BigDecimal::from_str("3").unwrap(),
            default_quantity.clone(),
        );
        let mut agg = AggregatedOrderbook::new();
        agg.merge(&ob1);
        agg.merge(&ob2);
        let summary = agg.finalize().unwrap();
        assert_eq!(summary.spread, 0_f64.to_string());
        assert_eq!(
            summary.asks,
            vec![
                Level {
                    exchange: "A".to_string(),
                    price: 1_f64.to_string(),
                    amount: 10_f64.to_string(),
                },
                Level {
                    exchange: "B".to_string(),
                    price: 1_f64.to_string(),
                    amount: 10_f64.to_string(),
                },
                Level {
                    exchange: "A".to_string(),
                    price: 2_f64.to_string(),
                    amount: 10_f64.to_string()
                },
                Level {
                    exchange: "B".to_string(),
                    price: 3_f64.to_string(),
                    amount: 10_f64.to_string(),
                },
            ]
        );
        assert_eq!(summary.bids.len(), 0);
    }
}
