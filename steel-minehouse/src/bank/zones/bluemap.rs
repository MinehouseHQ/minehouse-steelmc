//! The zones on the web map (the Fabric `ZoneBlueMap`): clickable shapes whose popup is the
//! card of the lot: its status, the tenant or the bid, when the auction ends and a link to
//! bid on the site.
//!
//! The Fabric mod hands the set to `BlueMap`. Here it goes into the same `markers.json`
//! files as the metro (`content/map_markers.rs`), as the marker set `minehouse-zones`.

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset, Utc};
use serde_json::{Value, json};

use super::{Zone, left, markup};
use crate::{
    bank::Bank,
    content::map_markers::{self, map_name},
};

/// The id of the marker set.
pub const MARKER_SET: &str = "minehouse-zones";

/// The height a shape is drawn at: just above the sea, inside the zone.
fn height(zone: &Zone) -> i32 {
    let sea = match zone.world.as_str() {
        "minecraft:the_nether" => 32,
        "minecraft:the_end" => 0,
        _ => 63,
    };
    (sea + 2).min(zone.y_max).max(zone.y_min)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `15.11 21:30`, Moscow time.
fn when(at: DateTime<Utc>) -> String {
    FixedOffset::east_opt(3 * 3600).map_or_else(String::new, |moscow| {
        at.with_timezone(&moscow).format("%-d.%m %H:%M").to_string()
    })
}

/// Where a zone's card is on the site, for the bid button. Absolute, because the map is
/// also opened straight from the address of the game server.
fn zone_url(site: &str, server_id: &str, zone: &Zone) -> String {
    let realm = if server_id == "main" {
        ""
    } else {
        "realm=test&"
    };
    format!(
        "{}/zones?{realm}zone={num}#zone-{num}",
        site.trim_end_matches('/'),
        num = zone.num
    )
}

/// `_top`, so inside the site's `/map` frame the link leaves the frame.
fn button(url: &str, color: &str, text: &str) -> String {
    format!(
        "<a href=\"{}\" target=\"_top\" style=\"position:relative;z-index:2;pointer-events:auto;\
         cursor:pointer;opacity:1;display:inline-block;margin-top:6px;padding:4px 10px;\
         border-radius:6px;background:{color};color:#111;text-decoration:none;\
         font-weight:600\">{text}</a>",
        escape(url)
    )
}

/// The popup of a zone, as HTML.
fn card(zone: &Zone, url: &str, now: DateTime<Utc>) -> String {
    let mut html = vec![format!(
        "<div style=\"padding:6px 4px;line-height:1.45;min-width:180px;pointer-events:auto\">\
         <b style=\"font-size:1.1em\">{}</b><br>",
        escape(&zone.name())
    )];
    if zone.common {
        html.push("Общая зона спавна</div>".to_owned());
        return html.concat();
    }
    html.push(format!(
        "<span style=\"opacity:.7\">{} блоков · высоты {}–{}</span><br>",
        zone.area(),
        zone.y_min,
        zone.y_max
    ));
    if let Some(price) = zone.renew_price {
        html.push(format!("Аренда {price} АР за 7 дней<br>"));
    }
    match (zone.status.as_str(), &zone.auction) {
        ("auction", Some(auction)) if !auction.started(now) => {
            html.push(format!(
                "<b style=\"color:#ff8a00\">Скоро аукцион</b>: старт {} МСК, через {}<br>\
                 Первая ставка от {} АР<br>{}",
                when(auction.starts_at),
                left(auction.starts_at, now),
                auction.start_price,
                button(url, "#ff8a00", "Подробнее")
            ));
        }
        ("auction", Some(auction)) => {
            html.push(format!(
                "<b style=\"color:#ffb000\">Аукцион</b> до {} МСК<br>",
                when(auction.ends_at)
            ));
            match auction.top {
                Some(top) => html.push(format!(
                    "Ставка <b>{top} АР</b> · {}<br>",
                    escape(auction.top_nickname.as_deref().unwrap_or_default())
                )),
                None => html.push(format!("Ставок нет, старт {} АР<br>", auction.start_price)),
            }
            html.push(format!(
                "Следующая ставка от {} АР<br>{}",
                auction.min_bid,
                button(url, "#ffb000", "Сделать ставку")
            ));
        }
        ("leased", _) => {
            html.push(format!(
                "Арендует <b>{}</b><br>",
                escape(zone.owner.as_deref().unwrap_or("игрок"))
            ));
            if let Some(until) = zone.lease_until {
                html.push(format!("до {} МСК<br>", when(until)));
            }
        }
        _ => html.push(format!(
            "<span style=\"color:#55e05a\">Свободна</span>. Аукцион объявит администрация, \
             старт {} АР<br>",
            zone.start_price
        )),
    }
    html.push("</div>".to_owned());
    html.concat()
}

fn rgba(rgb: i32, alpha: f64) -> Value {
    json!({
        "r": (rgb >> 16) & 0xFF,
        "g": (rgb >> 8) & 0xFF,
        "b": rgb & 0xFF,
        "a": alpha,
    })
}

fn marker(zone: &Zone, url: &str, now: DateTime<Utc>) -> Value {
    let rgb = markup::color(zone);
    let y = f64::from(height(zone));
    let corners: Vec<(f64, f64)> = zone
        .points
        .iter()
        .map(|(x, z)| (f64::from(*x) + 0.5, f64::from(*z) + 0.5))
        .collect();
    // The middle of the bounding box, as `BlueMap` centers a shape.
    let middle = |axis: fn(&(f64, f64)) -> f64| {
        let low = corners.iter().map(axis).fold(f64::INFINITY, f64::min);
        let high = corners.iter().map(axis).fold(f64::NEG_INFINITY, f64::max);
        f64::midpoint(low, high)
    };
    let shape: Vec<Value> = corners
        .iter()
        .map(|(x, z)| json!({ "x": x, "z": z }))
        .collect();
    json!({
        "type": "shape",
        "label": zone.name(),
        "detail": card(zone, url, now),
        "position": { "x": middle(|corner| corner.0), "y": y, "z": middle(|corner| corner.1) },
        "shape": shape,
        "shapeY": y,
        "link": Value::Null,
        "newTab": false,
        "depthTest": false,
        "lineWidth": 3,
        "lineColor": rgba(rgb, 0.95),
        "fillColor": rgba(rgb, if zone.common { 0.08 } else { 0.22 }),
        "sorting": 0,
        "listed": true,
        "minDistance": 0.0,
        "maxDistance": 10_000_000.0,
    })
}

/// The marker set of every map that has zones, by the name of the map.
#[must_use]
pub fn marker_sets(
    zones: &[Zone],
    site: &str,
    server_id: &str,
    now: DateTime<Utc>,
) -> BTreeMap<String, Value> {
    let mut maps: BTreeMap<String, serde_json::Map<String, Value>> = BTreeMap::new();
    for zone in zones {
        let url = zone_url(site, server_id, zone);
        maps.entry(map_name(&zone.world))
            .or_default()
            .insert(format!("zone/{}", zone.num), marker(zone, &url, now));
    }
    maps.into_iter()
        .map(|(map, markers)| {
            let set = json!({
                "label": "Зоны спавна",
                "toggleable": true,
                "defaultHidden": false,
                "sorting": 0,
                "markers": markers,
            });
            (map, set)
        })
        .collect()
}

impl Bank {
    /// Puts the zones on the web map, or takes them off while the zones are closed.
    pub(super) fn draw_zones_on_map(&self) {
        let sets = if self.zones_open() {
            marker_sets(
                &self.zones.all(),
                &self.config.api_url,
                &self.config.server_id,
                Utc::now(),
            )
        } else {
            BTreeMap::new()
        };
        map_markers::put_extra(MARKER_SET, sets);
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-15T12:00:00Z")
            .map_or_default(|time| time.with_timezone(&Utc))
    }

    fn zone(num: i32, status: &str, world: &str) -> Zone {
        let listed = json!({
            "id": format!("z-{num}"), "num": num, "kind": if num == 0 { "common" } else { "lot" },
            "world": world, "points": [[0, 0], [10, 0], [10, 4], [0, 4]], "y_min": 60,
            "y_max": 100, "start_price": 5, "status": status, "rent_price": 7,
        });
        Zone::parse(&listed).unwrap_or_else(|| unreachable!())
    }

    #[test]
    fn a_zone_is_a_shape_in_its_status_color() {
        let sets = marker_sets(
            &[
                zone(0, "free", "minecraft:overworld"),
                zone(3, "leased", "minecraft:overworld"),
                zone(4, "free", "minecraft:the_nether"),
            ],
            "https://minehouse.pro/",
            "main",
            now(),
        );
        assert_eq!(sets.len(), 2);
        let world = &sets["world"]["markers"];
        let lot = &world["zone/3"];
        assert_eq!(lot["type"], "shape");
        assert_eq!(lot["label"], "Зона 3");
        assert_eq!(lot["shape"][2], json!({ "x": 10.5, "z": 4.5 }));
        assert_eq!(lot["position"], json!({ "x": 5.5, "y": 65.0, "z": 2.5 }));
        assert_eq!(lot["shapeY"], 65.0);
        assert_eq!(
            lot["lineColor"],
            json!({ "r": 0xE0, "g": 0x48, "b": 0x48, "a": 0.95 })
        );
        assert_eq!(lot["fillColor"]["a"], 0.22);
        assert_eq!(world["zone/0"]["fillColor"]["a"], 0.08);
        // The Nether's sea is lower than the zone: the shape is at the zone's floor.
        let nether = &sets["world_the_nether"]["markers"]["zone/4"];
        assert_eq!(nether["shapeY"], 60.0);
    }

    #[test]
    fn the_card_says_what_the_zone_is_up_to() {
        let url = zone_url("https://minehouse.pro/", "test", &zone(3, "free", "w"));
        assert_eq!(url, "https://minehouse.pro/zones?realm=test&zone=3#zone-3");
        let main = zone_url("https://minehouse.pro", "main", &zone(3, "free", "w"));
        assert_eq!(main, "https://minehouse.pro/zones?zone=3#zone-3");

        let free = card(&zone(3, "free", "w"), &url, now());
        assert!(free.contains("40 блоков · высоты 60–100"), "{free}");
        assert!(free.contains("Аренда 7 АР за 7 дней"), "{free}");
        assert!(free.contains("Свободна"), "{free}");
        assert!(!free.contains("<a "), "{free}");

        let leased = Zone {
            owner: Some("<b>bob".to_owned()),
            lease_until: Some(now() + Duration::days(2)),
            ..zone(3, "leased", "w")
        };
        let leased = card(&leased, &url, now());
        assert!(leased.contains("Арендует <b>&lt;b&gt;bob</b>"), "{leased}");
        assert!(leased.contains("до 17.11 15:00 МСК"), "{leased}");

        let common = card(&zone(0, "free", "w"), &url, now());
        assert!(common.ends_with("Общая зона спавна</div>"), "{common}");
    }

    #[test]
    fn an_auction_has_a_button_to_the_site() {
        let at = |minutes: i64| (now() + Duration::minutes(minutes)).to_rfc3339();
        let listed = |starts: i64, top: Option<i64>| {
            let listed = json!({
                "id": "z-3", "num": 3, "kind": "lot", "world": "w",
                "points": [[0, 0], [10, 0], [10, 4], [0, 4]], "y_min": 60, "y_max": 100,
                "start_price": 5, "status": "auction",
                "auction": {
                    "id": "a-3", "starts_at": at(starts), "ends_at": at(90), "start_price": 5,
                    "top": top, "top_nickname": top.map(|_| "buyer"),
                    "min_bid": top.map_or(5, |top| top + 1), "bids": 0,
                },
            });
            Zone::parse(&listed).unwrap_or_else(|| unreachable!())
        };
        let url = "https://minehouse.pro/zones?zone=3#zone-3";
        let soon = card(&listed(30, None), url, now());
        assert!(
            soon.contains("Скоро аукцион</b>: старт 15.11 15:30 МСК, через 30 мин"),
            "{soon}"
        );
        assert!(soon.contains(">Подробнее</a>"), "{soon}");
        let open = card(&listed(-30, Some(12)), url, now());
        assert!(open.contains("Аукцион</b> до 15.11 16:30 МСК"), "{open}");
        assert!(open.contains("Ставка <b>12 АР</b> · buyer"), "{open}");
        assert!(open.contains("Следующая ставка от 13 АР"), "{open}");
        assert!(
            open.contains("href=\"https://minehouse.pro/zones?zone=3#zone-3\""),
            "{open}"
        );
        let quiet = card(&listed(-30, None), url, now());
        assert!(quiet.contains("Ставок нет, старт 5 АР"), "{quiet}");
    }
}
