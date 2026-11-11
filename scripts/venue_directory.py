#!/usr/bin/env python3
"""Build data/venue_directory.csv, the venue directory the AI venue scout searches.

Two sources, one format:

    python3 scripts/venue_directory.py sample            > data/venue_directory.csv   # fictional, reproducible
    python3 scripts/venue_directory.py import EXPORT.csv > data/venue_directory.csv   # anonymized discovery export

`import` reads a discovery export with the columns city, country, genres, collected_events_total (one venue per
row) and prepares it for a public demo:
  * names: a pseudonymized export (data_variant = pseudonymized_demo) keeps its pseudonyms ("Demo Venue 207");
    an export without names gets made-up ones. An export with real names is refused;
  * other identifying columns (instagram_id, ...) are dropped; rooms, capacity and prices are made up (the
    export has none);
  * show counts are rounded to the nearest 5 and capped at 50;
  * `activity` (0-100) is the percentile of the rounded show count across the directory.
Activity says how many shows the discovery data has on record for a venue. It is not live popularity.
A private export must never be committed.
"""
import csv
import random
import re
import sys

SEED = 20261111

COLUMNS = ["slug", "name", "city", "country", "region", "tz_offset_minutes", "kind", "genres", "capacity",
           "shows_on_record", "activity", "source"]

# Broad genre categories (the discovery data's own classification).
CATEGORY = {
    "electronic/dj": "Electronic / DJ",
    "rock/indie": "Rock / Indie",
    "vocal/soul": "Vocal / Soul",
    "pop/hip-hop": "Pop / Hip-hop",
    "jazz": "Jazz",
    "techno/house": "Techno / House",
    "house/techno": "Techno / House",
    "open format/edm": "Open format / EDM",
}

REGION = {
    "KR": "East Asia", "JP": "East Asia", "TW": "East Asia", "HK": "East Asia",
    "TH": "Southeast Asia", "SG": "Southeast Asia", "ID": "Southeast Asia", "PH": "Southeast Asia",
    "MY": "Southeast Asia", "VN": "Southeast Asia",
    "AU": "Oceania",
    "US": "North America", "CA": "North America",
    "MX": "Latin America", "BR": "Latin America",
    "GB": "Europe", "DE": "Europe", "NL": "Europe", "ES": "Europe", "IT": "Europe", "FR": "Europe",
}

# city -> (UTC offset in minutes in Nov-Dec, slug suffix)
CITY = {
    "Seoul": (540, "sel"), "Busan": (540, "pus"), "Tokyo": (540, "tyo"), "Osaka": (540, "osa"),
    "Taipei": (480, "tpe"), "Hong Kong": (480, "hkg"), "Bangkok": (420, "bkk"), "Singapore": (480, "sin"),
    "Jakarta": (420, "jkt"), "Manila": (480, "mnl"), "Kuala Lumpur": (480, "kul"), "Ho Chi Minh City": (420, "sgn"),
    "Sydney": (660, "syd"), "Melbourne": (660, "mel"),
    "Los Angeles": (-480, "lax"), "San Francisco/Oakland": (-480, "sfo"), "Chicago": (-360, "chi"),
    "Detroit": (-300, "dtw"), "Miami": (-300, "mia"), "New York City": (-300, "nyc"), "New York": (-300, "nyc"),
    "Toronto": (-300, "yyz"), "Mexico City": (-360, "mex"), "Sao Paulo": (-180, "sao"), "São Paulo": (-180, "sao"),
    "London": (0, "lon"), "Berlin": (60, "ber"), "Amsterdam": (60, "ams"), "Paris": (60, "par"),
    "Barcelona": (60, "bcn"), "Madrid": (60, "mad"), "Milan": (60, "mil"),
}

ADJ = ["Velvet", "Copper", "Paper", "Hollow", "Signal", "Tin", "Glass", "Moth", "Ember", "Fog", "Cobalt", "Orbit",
       "Pocket", "Rust", "Silver", "Tidal", "Static", "Lantern", "Neon", "Amber", "Indigo", "Marble", "Coral",
       "Iron", "Cinder", "Halo", "Prism", "Saffron", "Willow", "Lumen", "Juniper", "Basalt", "Kite", "Comet",
       "Ivory", "Slate", "Hazel", "Russet", "Opal", "Cedar"]
NOUN = ["Room", "Cellar", "Hall", "Garage", "Loft", "Den", "Yard", "Hangar", "Parlor", "Vault", "Works",
        "Theater", "Lounge", "Terrace", "Station", "Dock", "Foundry", "Annex", "Arcade", "Depot"]

# The three hand-made sample venue cards: their names, rooms and prices come from the database seed.
FEATURED = [
    {"slug": "blue-lantern", "name": "Blue Lantern Hall", "city": "Seoul", "country": "KR", "kind": "Live house",
     "genres": ["Rock / Indie", "Electronic / DJ"], "capacity": 500, "shows": 45},
    {"slug": "harbor-room", "name": "Harbor Room", "city": "Busan", "country": "KR", "kind": "Bar stage",
     "genres": ["Vocal / Soul", "Jazz"], "capacity": 250, "shows": 25},
    {"slug": "grey-matter", "name": "Grey Matter", "city": "Tokyo", "country": "JP", "kind": "Club",
     "genres": ["Electronic / DJ", "Techno / House"], "capacity": 300, "shows": 40},
]


def slugify(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")


def category(g: str) -> str:
    g = g.strip()
    return CATEGORY.get(g.lower(), " / ".join(p.strip().capitalize() for p in g.split("/")))


def percentile_ranks(values):
    """0..1 rank of each value among all values (ties share the average rank)."""
    order = sorted(range(len(values)), key=lambda i: values[i])
    ranks = [0.0] * len(values)
    i = 0
    while i < len(order):
        j = i
        while j + 1 < len(order) and values[order[j + 1]] == values[order[i]]:
            j += 1
        avg = (i + j) / 2 / max(1, len(values) - 1)
        for k in range(i, j + 1):
            ranks[order[k]] = avg
        i = j + 1
    return ranks


def add_activity(rows):
    for r, p in zip(rows, percentile_ranks([r["shows_on_record"] for r in rows])):
        r["activity"] = int(round(100 * p))


def round5(n: int) -> int:
    return max(5, int(5 * round(n / 5)))


def name_maker(rnd):
    used = {f["name"] for f in FEATURED}

    def make():
        for _ in range(10_000):
            name = f"{rnd.choice(ADJ)} {rnd.choice(NOUN)}"
            if name not in used:
                used.add(name)
                return name
        raise SystemExit("ran out of venue names")
    return make


def demo_capacity(rnd) -> int:
    base = rnd.choices([120, 200, 300, 450, 600, 800, 1200], [12, 20, 22, 18, 12, 10, 6])[0]
    return int(round(base * rnd.uniform(0.85, 1.15), -1))


def featured_rows():
    out = []
    for f in FEATURED:
        tz, _ = CITY[f["city"]]
        out.append({
            "slug": f["slug"], "name": f["name"], "city": f["city"], "country": f["country"],
            "region": REGION[f["country"]], "tz_offset_minutes": tz, "kind": f["kind"],
            "genres": "|".join(f["genres"]), "capacity": f["capacity"], "shows_on_record": f["shows"],
            "source": "sample",
        })
    return out


def sample_rows():
    """A fictional directory: 18 cities, 7-9 venues each."""
    rnd = random.Random(SEED)
    name = name_maker(rnd)
    scenes = {
        "Seoul": ["Rock / Indie", "Pop / Hip-hop", "Electronic / DJ"], "Busan": ["Rock / Indie", "Pop / Hip-hop"],
        "Tokyo": ["Rock / Indie", "Electronic / DJ", "Jazz"], "Osaka": ["Rock / Indie", "Jazz", "Electronic / DJ"],
        "Taipei": ["Rock / Indie", "Electronic / DJ"], "Hong Kong": ["Electronic / DJ", "Jazz"],
        "Bangkok": ["Rock / Indie", "Electronic / DJ"], "Singapore": ["Electronic / DJ", "Vocal / Soul", "Jazz"],
        "Jakarta": ["Rock / Indie", "Pop / Hip-hop"], "Manila": ["Rock / Indie", "Vocal / Soul", "Pop / Hip-hop"],
        "Kuala Lumpur": ["Rock / Indie", "Electronic / DJ"], "Ho Chi Minh City": ["Electronic / DJ", "Pop / Hip-hop"],
        "Sydney": ["Rock / Indie", "Electronic / DJ"], "Melbourne": ["Rock / Indie", "Vocal / Soul"],
        "Los Angeles": ["Electronic / DJ", "Pop / Hip-hop", "Rock / Indie"], "New York City": ["Jazz", "Pop / Hip-hop", "Rock / Indie"],
        "London": ["Rock / Indie", "Electronic / DJ"], "Berlin": ["Techno / House", "Electronic / DJ"],
    }
    countries = {"Seoul": "KR", "Busan": "KR", "Tokyo": "JP", "Osaka": "JP", "Taipei": "TW", "Hong Kong": "HK",
                 "Bangkok": "TH", "Singapore": "SG", "Jakarta": "ID", "Manila": "PH", "Kuala Lumpur": "MY",
                 "Ho Chi Minh City": "VN", "Sydney": "AU", "Melbourne": "AU", "Los Angeles": "US",
                 "New York City": "US", "London": "GB", "Berlin": "DE"}
    kinds = ["Live house", "Club", "Bar stage", "Hall", "Rooftop", "Warehouse"]
    rows = []
    for city, scene in scenes.items():
        tz, code = CITY[city]
        for _ in range(rnd.randint(7, 9)):
            n = name()
            genres = rnd.sample(scene, k=min(len(scene), rnd.choice([1, 1, 2])))
            rows.append({
                "slug": f"{slugify(n)}-{code}", "name": n, "city": city, "country": countries[city],
                "region": REGION[countries[city]], "tz_offset_minutes": tz,
                "kind": rnd.choices(kinds, [5, 4, 3, 2, 2, 2])[0], "genres": "|".join(genres),
                "capacity": demo_capacity(rnd), "shows_on_record": round5(int(rnd.lognormvariate(3.4, 0.35))),
                "source": "sample",
            })
    rows.extend(featured_rows())
    add_activity(rows)
    return rows


def import_rows(path):
    """An anonymized directory from a private discovery export (see the module docstring)."""
    rnd = random.Random(SEED)
    name = name_maker(rnd)
    rows = []
    with open(path, encoding="utf-8") as f:
        for src in csv.DictReader(f):
            if src.get("venue_name") and src.get("data_variant") != "pseudonymized_demo":
                raise SystemExit("this export has real venue names: use the pseudonymized (PUBLIC) export")
            city, country = src["city"].strip(), src["country"].strip().upper()
            if city not in CITY:
                raise SystemExit(f"unknown city {city!r}: add its UTC offset to CITY")
            tz, code = CITY[city]
            genres = []
            for g in re.split(r"[,;|]", src["genres"]):
                if g.strip() and category(g) not in genres:
                    genres.append(category(g))
            n = src["venue_name"].strip() if src.get("venue_name") else name()
            rows.append({
                "slug": f"{slugify(n)}-{code}", "name": n, "city": city, "country": country,
                "region": REGION.get(country, ""), "tz_offset_minutes": tz, "kind": "",
                "genres": "|".join(genres), "capacity": demo_capacity(rnd),
                # capped at 50 so that no single outlier count stays unique
                "shows_on_record": min(50, round5(int(src["collected_events_total"]))), "source": "discovery",
            })
    rows.extend(featured_rows())
    add_activity(rows)
    return rows


def write_csv(rows, out=sys.stdout):
    rows = sorted(rows, key=lambda r: (r["region"], r["city"], -r["activity"], r["name"]))
    w = csv.DictWriter(out, fieldnames=COLUMNS, lineterminator="\n")
    w.writeheader()
    for r in rows:
        if any("," in str(r[k]) for k in COLUMNS):
            raise SystemExit(f"comma in {r['slug']}: the loader splits on commas")
        w.writerow({k: r[k] for k in COLUMNS})


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "sample":
        write_csv(sample_rows())
    elif len(sys.argv) == 3 and sys.argv[1] == "import":
        write_csv(import_rows(sys.argv[2]))
    else:
        raise SystemExit(__doc__)
