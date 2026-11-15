//! One zone of the spawn island as the site lists it (`zones` in minehouse-landing
//! `bank-mod/README.md`).

use chrono::{DateTime, Utc};
use serde_json::Value;

/// The auction of a zone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Auction {
    /// The id the site knows the auction by.
    pub id: String,
    /// When an admin announced it, if the site says.
    pub announced_at: Option<DateTime<Utc>>,
    /// When bidding opens: an auction can be announced in advance.
    pub starts_at: DateTime<Utc>,
    /// When bidding ends.
    pub ends_at: DateTime<Utc>,
    /// The least first bid, АР.
    pub start_price: i64,
    /// The highest bid so far.
    pub top: Option<i64>,
    /// Who made it.
    pub top_nickname: Option<String>,
    /// The least next bid.
    pub min_bid: i64,
    /// How many bids were made.
    pub bids: i64,
}

impl Auction {
    /// Whether bidding is open.
    #[must_use]
    pub fn started(&self, now: DateTime<Utc>) -> bool {
        self.starts_at <= now
    }
}

/// A zone: the common circle or a lot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Zone {
    /// The id the site knows the zone by.
    pub id: String,
    /// Its number, what players and admins call it by.
    pub num: i32,
    /// The shared zone of the spawn, not a lot.
    pub common: bool,
    /// Its own name, if it has one.
    pub title: Option<String>,
    /// The world it is in, `minecraft:overworld`.
    pub world: String,
    /// The vertices of its outline, block x and z.
    pub points: Vec<(i32, i32)>,
    /// The lowest block height in it.
    pub y_min: i32,
    /// The highest block height in it.
    pub y_max: i32,
    /// What its auction starts from.
    pub start_price: i64,
    /// `free`, `auction` or `leased`.
    pub status: String,
    /// The nickname of the tenant.
    pub owner: Option<String>,
    /// The player who asked rents it.
    pub mine: bool,
    /// When the lease ends.
    pub lease_until: Option<DateTime<Utc>>,
    /// What seven more days cost.
    pub renew_price: Option<i64>,
    /// Its auction, announced or running.
    pub auction: Option<Auction>,
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_owned)
}

fn time(value: &Value, key: &str) -> Option<DateTime<Utc>> {
    let text = value.get(key)?.as_str()?;
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

fn int(value: &Value, key: &str) -> Option<i32> {
    i32::try_from(value.get(key)?.as_i64()?).ok()
}

fn auction(value: &Value) -> Option<Auction> {
    Some(Auction {
        id: text(value, "id")?,
        announced_at: time(value, "announced_at"),
        // No start: it has always been open.
        starts_at: time(value, "starts_at").unwrap_or(DateTime::UNIX_EPOCH),
        ends_at: time(value, "ends_at")?,
        start_price: value.get("start_price")?.as_i64()?,
        top: value.get("top").and_then(Value::as_i64),
        top_nickname: text(value, "top_nickname"),
        min_bid: value.get("min_bid")?.as_i64()?,
        bids: value.get("bids")?.as_i64()?,
    })
}

/// «1 д 4 ч», «3 ч 12 мин», «5 мин 20 сек», «40 сек».
#[must_use]
pub fn left(until: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let total = (until - now).num_seconds();
    if total <= 0 {
        return "сейчас".to_owned();
    }
    let (days, hours) = (total / 86_400, total % 86_400 / 3_600);
    let (minutes, seconds) = (total % 3_600 / 60, total % 60);
    if days > 0 {
        format!("{days} д {hours} ч")
    } else if hours > 0 {
        format!("{hours} ч {minutes} мин")
    } else if minutes > 0 {
        format!("{minutes} мин {seconds} сек")
    } else {
        format!("{seconds} сек")
    }
}

impl Zone {
    /// Reads a zone of a `zones` answer. None when a field it can't do without is missing.
    #[must_use]
    pub fn parse(value: &Value) -> Option<Self> {
        let points = value
            .get("points")?
            .as_array()?
            .iter()
            .map(|point| {
                let x = i32::try_from(point.get(0)?.as_i64()?).ok()?;
                let z = i32::try_from(point.get(1)?.as_i64()?).ok()?;
                Some((x, z))
            })
            .collect::<Option<Vec<_>>>()?;
        let renew_price = value
            .get("rent_price")
            .or_else(|| value.get("renew_price"))
            .and_then(Value::as_i64);
        Some(Self {
            id: text(value, "id")?,
            num: int(value, "num")?,
            common: value.get("kind")?.as_str()? == "common",
            title: text(value, "title"),
            world: text(value, "world")?,
            points,
            y_min: int(value, "y_min")?,
            y_max: int(value, "y_max")?,
            start_price: value.get("start_price")?.as_i64()?,
            status: text(value, "status")?,
            owner: text(value, "owner"),
            mine: value.get("mine").and_then(Value::as_bool).unwrap_or(false),
            lease_until: time(value, "lease_until"),
            renew_price,
            auction: value
                .get("auction")
                .filter(|auction| auction.is_object())
                .and_then(auction),
        })
    }

    /// Its own name, «Спавн» or «Зона 3».
    #[must_use]
    pub fn name(&self) -> String {
        if let Some(title) = &self.title {
            return title.clone();
        }
        if self.common {
            "Спавн".to_owned()
        } else {
            format!("Зона {}", self.num)
        }
    }

    fn auction_of_status(&self) -> Option<&Auction> {
        self.auction.as_ref().filter(|_| self.status == "auction")
    }

    /// An auction with bidding open.
    #[must_use]
    pub fn auction_open(&self, now: DateTime<Utc>) -> bool {
        self.auction_of_status()
            .is_some_and(|auction| auction.started(now))
    }

    /// An auction announced, bidding not open yet.
    #[must_use]
    pub fn auction_soon(&self, now: DateTime<Utc>) -> bool {
        self.auction_of_status()
            .is_some_and(|auction| !auction.started(now))
    }

    /// Someone rents it.
    #[must_use]
    pub fn leased(&self) -> bool {
        self.status == "leased"
    }

    /// Whether a position is in the zone: point-in-polygon on block centers, then the height
    /// band.
    #[must_use]
    pub fn contains(&self, world: &str, x: f64, y: f64, z: f64) -> bool {
        self.world == world
            && y >= f64::from(self.y_min)
            && y < f64::from(self.y_max) + 1.0
            && self.contains_xz(x, z)
    }

    /// Whether a column is in the outline.
    #[must_use]
    pub fn contains_xz(&self, x: f64, z: f64) -> bool {
        let Some(&last) = self.points.last() else {
            return false;
        };
        let center = |(x, z): (i32, i32)| (f64::from(x) + 0.5, f64::from(z) + 0.5);
        let mut inside = false;
        let mut previous = center(last);
        for &point in &self.points {
            let (xi, zi) = center(point);
            let (xj, zj) = previous;
            if (zi > z) != (zj > z) && x < (xj - xi) * (z - zi) / (zj - zi) + xi {
                inside = !inside;
            }
            previous = (xi, zi);
        }
        inside
    }

    /// The area of the outline, blocks.
    #[must_use]
    pub fn area(&self) -> i64 {
        let Some(&last) = self.points.last() else {
            return 0;
        };
        let mut twice = 0_i64;
        let mut previous = last;
        for &(x, z) in &self.points {
            twice += i64::from(previous.0) * i64::from(z) - i64::from(x) * i64::from(previous.1);
            previous = (x, z);
        }
        (twice / 2).abs()
    }

    /// The lines of the zone's card.
    #[must_use]
    pub fn describe(&self, now: DateTime<Utc>) -> Vec<String> {
        if self.common {
            return vec!["Общая зона спавна.".to_owned()];
        }
        let mut lines = vec![format!(
            "Площадь: {} блоков, высоты {}–{}",
            self.area(),
            self.y_min,
            self.y_max
        )];
        if let Some(price) = self.renew_price {
            lines.push(format!("Аренда: {price} АР за 7 дней"));
        }
        match (self.status.as_str(), &self.auction) {
            ("auction", Some(auction)) if !auction.started(now) => {
                lines.push(format!(
                    "Аукцион начнётся через {}",
                    left(auction.starts_at, now)
                ));
                lines.push(format!("Первая ставка от {} АР", auction.start_price));
                lines.push(format!(
                    "Продлится до конца через {}",
                    left(auction.ends_at, now)
                ));
            }
            ("auction", Some(auction)) => {
                lines.push(format!("Аукцион, осталось {}", left(auction.ends_at, now)));
                lines.push(match (auction.top, &auction.top_nickname) {
                    (Some(top), Some(nick)) => format!("Ставка {top} АР · {nick}"),
                    (Some(top), None) => format!("Ставка {top} АР"),
                    (None, _) => format!("Ставок нет, старт {} АР", auction.start_price),
                });
                lines.push(format!("Следующая ставка от {} АР", auction.min_bid));
            }
            ("leased", _) => {
                lines.push(format!(
                    "Арендует {}",
                    self.owner.as_deref().unwrap_or("игрок")
                ));
                if let Some(until) = self.lease_until {
                    lines.push(format!("Ещё {}", left(until, now)));
                    if !self.mine {
                        lines.push(format!("Не продлят — аукцион через {}", left(until, now)));
                    }
                }
            }
            _ => lines.push(format!(
                "Свободна: аукцион объявит администрация, старт {} АР",
                self.start_price
            )),
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use serde_json::json;

    use super::*;

    fn now() -> DateTime<Utc> {
        time(&json!({"at": "2026-11-15T12:00:00Z"}), "at").unwrap_or_default()
    }

    fn listed() -> Value {
        json!({
            "id": "z-3", "num": 3, "kind": "lot", "title": null, "world": "minecraft:overworld",
            "points": [[0, 0], [10, 0], [10, 10], [0, 10]], "y_min": 60, "y_max": 100,
            "start_price": 5, "status": "auction", "owner": null, "mine": false,
            "lease_until": null, "rent_price": 7,
            "auction": {
                "id": "a-1", "announced_at": null, "starts_at": "2026-11-15T11:00:00Z",
                "ends_at": "2026-11-15T13:30:00Z", "start_price": 5, "top": 12,
                "top_nickname": "buyer", "min_bid": 13, "bids": 4,
            },
        })
    }

    #[test]
    fn a_zone_of_the_site_is_read() {
        let zone = Zone::parse(&listed()).unwrap_or_else(|| unreachable!());
        assert_eq!(zone.name(), "Зона 3");
        assert_eq!(zone.area(), 100);
        assert_eq!(zone.renew_price, Some(7));
        assert!(zone.auction_open(now()));
        assert!(!zone.auction_soon(now()));
        assert_eq!(
            zone.describe(now()),
            vec![
                "Площадь: 100 блоков, высоты 60–100",
                "Аренда: 7 АР за 7 дней",
                "Аукцион, осталось 1 ч 30 мин",
                "Ставка 12 АР · buyer",
                "Следующая ставка от 13 АР",
            ]
        );
        let mut broken = listed();
        broken["points"] = json!("none");
        assert_eq!(Zone::parse(&broken), None);
    }

    #[test]
    fn the_older_name_of_the_rent_price_is_read_too() {
        let mut old = listed();
        old["rent_price"] = Value::Null;
        old["renew_price"] = json!(9);
        // A null `rent_price` is still there: the site sends one or the other.
        if let Some(fields) = old.as_object_mut() {
            fields.remove("rent_price");
        }
        assert_eq!(Zone::parse(&old).and_then(|zone| zone.renew_price), Some(9));
    }

    #[test]
    fn a_position_is_in_a_zone_by_block_centers_and_height() {
        let zone = Zone::parse(&listed()).unwrap_or_else(|| unreachable!());
        let world = "minecraft:overworld";
        assert!(zone.contains(world, 5.0, 60.0, 5.0));
        assert!(zone.contains(world, 5.0, 100.9, 5.0));
        assert!(!zone.contains(world, 5.0, 101.0, 5.0));
        assert!(!zone.contains(world, 5.0, 59.9, 5.0));
        // The outline runs through the centers of the corner blocks.
        assert!(!zone.contains(world, 0.2, 70.0, 5.0));
        assert!(zone.contains(world, 0.7, 70.0, 5.0));
        assert!(!zone.contains(world, 10.7, 70.0, 5.0));
        assert!(!zone.contains("minecraft:the_end", 5.0, 70.0, 5.0));
    }

    #[test]
    fn time_left_is_said_in_two_units() {
        let now = now();
        assert_eq!(left(now + Duration::seconds(40), now), "40 сек");
        assert_eq!(left(now + Duration::seconds(320), now), "5 мин 20 сек");
        assert_eq!(left(now + Duration::minutes(192), now), "3 ч 12 мин");
        assert_eq!(left(now + Duration::hours(28), now), "1 д 4 ч");
        assert_eq!(left(now - Duration::seconds(5), now), "сейчас");
    }

    #[test]
    fn a_leased_zone_says_who_rents_it_and_for_how_long() {
        let mut leased = listed();
        leased["status"] = json!("leased");
        leased["owner"] = json!("seller");
        leased["lease_until"] = json!("2026-11-17T12:00:00Z");
        leased["auction"] = Value::Null;
        let zone = Zone::parse(&leased).unwrap_or_else(|| unreachable!());
        assert!(zone.leased());
        assert!(!zone.auction_open(now()));
        assert_eq!(
            zone.describe(now())[2..],
            [
                "Арендует seller".to_owned(),
                "Ещё 2 д 0 ч".to_owned(),
                "Не продлят — аукцион через 2 д 0 ч".to_owned(),
            ]
        );
    }
}
