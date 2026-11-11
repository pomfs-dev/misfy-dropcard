//! The venue directory the AI venue scout searches: `data/venue_directory.csv`, made by
//! `scripts/venue_directory.py` (fictional sample data, or an anonymized export of MiSFY's discovery data),
//! loaded into `venues` with one bookable room per venue. Activity (0-100) is precomputed in the file: the
//! percentile of the shows the discovery data has on record for the venue.

use anyhow::{bail, Context, Result};
use sqlx::PgPool;

const CSV: &str = include_str!("../data/venue_directory.csv");
const FEATURED: [&str; 3] = ["blue-lantern", "harbor-room", "grey-matter"];

struct Row {
    slug: String,
    name: String,
    city: String,
    country: String,
    region: String,
    tz_offset_minutes: i32,
    kind: Option<String>,
    genres: Vec<String>,
    capacity: i32,
    shows_on_record: i32,
    activity: i32,
    source: String,
}

/// The directory file: `VENUE_DIRECTORY_CSV` if set (an operator's own export, kept out of the repository),
/// otherwise the one built into the binary.
fn source() -> Result<String> {
    match std::env::var("VENUE_DIRECTORY_CSV") {
        Ok(path) if !path.trim().is_empty() => {
            std::fs::read_to_string(path.trim()).with_context(|| format!("reading VENUE_DIRECTORY_CSV={path}"))
        }
        _ => Ok(CSV.to_string()),
    }
}

fn parse(csv: &str) -> Result<Vec<Row>> {
    let mut lines = csv.lines();
    let header: Vec<&str> = lines.next().context("the venue directory is empty")?.split(',').collect();
    let col = |name: &str| header.iter().position(|c| *c == name).with_context(|| format!("column {name} is missing"));
    let (c_slug, c_name, c_city, c_country, c_region, c_tz, c_kind, c_genres, c_cap, c_shows, c_activity, c_src) = (
        col("slug")?,
        col("name")?,
        col("city")?,
        col("country")?,
        col("region")?,
        col("tz_offset_minutes")?,
        col("kind")?,
        col("genres")?,
        col("capacity")?,
        col("shows_on_record")?,
        col("activity")?,
        col("source")?,
    );
    let mut rows = Vec::new();
    for (n, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(',').collect();
        if f.len() != header.len() {
            bail!("venue directory line {}: expected {} fields, found {}", n + 2, header.len(), f.len());
        }
        let num = |i: usize| f[i].trim().parse::<i32>().with_context(|| format!("line {}: bad number {:?}", n + 2, f[i]));
        rows.push(Row {
            slug: f[c_slug].trim().to_string(),
            name: f[c_name].trim().to_string(),
            city: f[c_city].trim().to_string(),
            country: f[c_country].trim().to_string(),
            region: f[c_region].trim().to_string(),
            tz_offset_minutes: num(c_tz)?,
            kind: Some(f[c_kind].trim().to_string()).filter(|k| !k.is_empty()),
            genres: f[c_genres].split('|').map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect(),
            capacity: num(c_cap)?,
            shows_on_record: num(c_shows)?,
            activity: num(c_activity)?.clamp(0, 100),
            source: f[c_src].trim().to_string(),
        });
    }
    Ok(rows)
}

/// The day rate of a directory venue's room: $200 + $5 per guest, rounded to $50 (in cents).
pub fn day_rate_cents(capacity: i32) -> i64 {
    let dollars = 200 + 5 * capacity.max(0) as i64;
    (dollars + 25) / 50 * 50 * 100
}

/// Upserts the directory. Idempotent; the featured sample cards keep their name, rooms and prices and only
/// get the scout's signals. Venues dropped from the file stay (bookings may point at them) but leave the search.
pub async fn load(db: &PgPool) -> Result<usize> {
    let rows = parse(&source()?)?;
    let mut tx = db.begin().await?;
    sqlx::query("UPDATE venues SET featured = (slug = ANY($1))")
        .bind(&FEATURED[..])
        .execute(&mut *tx)
        .await?;
    let mut slugs = Vec::with_capacity(rows.len());
    for r in &rows {
        let cap_min = ((r.capacity as f64) * 0.4).round().max(10.0) as i32;
        let genre_list = r.genres.join(", ");
        let kind = r.kind.clone().unwrap_or_else(|| "Live venue".to_string());
        let tagline = format!("{kind} in {}", r.city);
        let description = format!(
            "A {} in {} with a room for up to {} guests. Mostly {}. About {} shows on record in MiSFY's discovery data.",
            kind.to_lowercase(),
            r.city,
            r.capacity,
            genre_list,
            r.shows_on_record
        );
        let mut tags = vec![kind.clone()];
        tags.extend(r.genres.iter().take(2).cloned());
        let (venue_id, featured): (uuid::Uuid, bool) = sqlx::query_as(
            "INSERT INTO venues (slug, name, city, country, tagline, description, tags, capacity_band, tz_offset_minutes,
                                 payout_email, featured, region, kind, genres, shows_on_record, activity, data_source)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'venue@sample.dropcard.test', false, $10, $11, $12, $13, $14, $15)
             ON CONFLICT (slug) DO UPDATE SET
                name = CASE WHEN venues.featured THEN venues.name ELSE EXCLUDED.name END,
                city = CASE WHEN venues.featured THEN venues.city ELSE EXCLUDED.city END,
                country = CASE WHEN venues.featured THEN venues.country ELSE EXCLUDED.country END,
                tagline = CASE WHEN venues.featured THEN venues.tagline ELSE EXCLUDED.tagline END,
                description = CASE WHEN venues.featured THEN venues.description ELSE EXCLUDED.description END,
                tags = CASE WHEN venues.featured THEN venues.tags ELSE EXCLUDED.tags END,
                capacity_band = CASE WHEN venues.featured THEN venues.capacity_band ELSE EXCLUDED.capacity_band END,
                tz_offset_minutes = CASE WHEN venues.featured THEN venues.tz_offset_minutes ELSE EXCLUDED.tz_offset_minutes END,
                region = EXCLUDED.region, kind = EXCLUDED.kind, genres = EXCLUDED.genres,
                shows_on_record = EXCLUDED.shows_on_record, activity = EXCLUDED.activity,
                data_source = EXCLUDED.data_source
             RETURNING id, featured",
        )
        .bind(&r.slug)
        .bind(&r.name)
        .bind(&r.city)
        .bind(&r.country)
        .bind(&tagline)
        .bind(&description)
        .bind(&tags)
        .bind(format!("{cap_min}–{}", r.capacity))
        .bind(r.tz_offset_minutes)
        .bind(&r.region)
        .bind(&r.kind)
        .bind(&r.genres)
        .bind(r.shows_on_record)
        .bind(r.activity)
        .bind(&r.source)
        .fetch_one(&mut *tx)
        .await
        .with_context(|| format!("loading venue {}", r.slug))?;
        if !featured {
            sqlx::query(
                "INSERT INTO spaces (venue_id, name, capacity_min, capacity_max, day_rate_cents, sort_order)
                 VALUES ($1, 'Main room', $2, $3, $4, 1)
                 ON CONFLICT (venue_id, name) DO UPDATE
                    SET capacity_min = EXCLUDED.capacity_min, capacity_max = EXCLUDED.capacity_max,
                        day_rate_cents = EXCLUDED.day_rate_cents",
            )
            .bind(venue_id)
            .bind(cap_min)
            .bind(r.capacity)
            .bind(day_rate_cents(r.capacity))
            .execute(&mut *tx)
            .await?;
        }
        slugs.push(r.slug.clone());
    }
    sqlx::query("UPDATE venues SET activity = NULL WHERE slug <> ALL($1)")
        .bind(&slugs)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_directory_parses() {
        let rows = parse(CSV).expect("data/venue_directory.csv parses");
        assert!(rows.len() >= 50, "expected a real directory, got {} rows", rows.len());
        for f in FEATURED {
            assert!(rows.iter().any(|r| r.slug == f), "{f} is missing from the directory");
        }
        let mut slugs: Vec<_> = rows.iter().map(|r| r.slug.as_str()).collect();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), rows.len(), "slugs must be unique");
        assert!(rows.iter().all(|r| !r.genres.is_empty() && r.capacity > 0));
    }

    #[test]
    fn day_rate_rounds_to_fifty_dollars() {
        assert_eq!(day_rate_cents(300), 170_000);
        assert_eq!(day_rate_cents(90), 65_000);
        assert_eq!(day_rate_cents(1450), 745_000);
    }
}
