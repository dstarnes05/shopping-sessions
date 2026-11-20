mod input;
mod output;

use std::{
    collections::{hash_map::Entry, HashMap},
    env,
    error::Error,
    fs,
    hash::{BuildHasher, RandomState},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::Mutex,
    time::Instant,
};

use input::{decimal, fields, files, rows, timestamp, DAY, WEEK};
use rayon::prelude::*;

type Bytes = Box<[u8]>;
type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Default)]
struct People {
    ids: HashMap<Bytes, usize>,
    names: Vec<Bytes>,
    parents: Vec<usize>,
    sizes: Vec<usize>,
    minimum: Vec<usize>,
}

impl People {
    fn intern(&mut self, name: &[u8]) -> usize {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let id = self.names.len();
        self.ids.insert(name.into(), id);
        self.names.push(name.into());
        self.parents.push(id);
        self.sizes.push(1);
        self.minimum.push(id);
        id
    }

    fn root(&mut self, mut id: usize) -> usize {
        while self.parents[id] != id {
            self.parents[id] = self.parents[self.parents[id]];
            id = self.parents[id];
        }
        id
    }

    fn join(&mut self, left: &[u8], right: &[u8]) {
        let left = self.intern(left);
        let right = self.intern(right);
        let (mut left, mut right) = (self.root(left), self.root(right));
        if left == right {
            return;
        }
        if self.sizes[left] < self.sizes[right] {
            std::mem::swap(&mut left, &mut right);
        }
        self.parents[right] = left;
        self.sizes[left] += self.sizes[right];
        if self.names[self.minimum[right]] < self.names[self.minimum[left]] {
            self.minimum[left] = self.minimum[right];
        }
    }

    fn finish(mut self) -> HashMap<Bytes, Bytes> {
        let canonical: Vec<_> = (0..self.names.len())
            .map(|id| {
                let root = self.root(id);
                self.minimum[root]
            })
            .collect();
        self.ids
            .into_iter()
            .filter_map(|(name, id)| {
                (id != canonical[id]).then(|| (name, self.names[canonical[id]].clone()))
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
struct Event {
    time: i64,
    revenue: u32,
    category: u32,
}

struct Click {
    time: i64,
    campaign: usize,
    id: Bytes,
}

#[derive(Default)]
struct UserDay {
    sessions: i64,
    events: i64,
    revenue: i64,
    categories: HashMap<u32, i64>,
}

#[derive(Clone, Copy, Default)]
struct CampaignDay {
    sessions: i64,
    purchasing: i64,
    revenue: i64,
    assists: i64,
}

struct Candidate {
    ingest: i64,
    valid: Option<(Bytes, Event)>,
}

struct Shards<T> {
    maps: Vec<Mutex<HashMap<Bytes, T>>>,
    hash: RandomState,
}

const SHARDS: usize = 256;

impl<T> Shards<T> {
    fn new() -> Self {
        Self {
            maps: (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect(),
            hash: RandomState::new(),
        }
    }

    fn update(&self, id: &[u8], update: impl FnOnce(&mut HashMap<Bytes, T>)) {
        let shard = self.hash.hash_one(id) as usize % SHARDS;
        update(&mut self.maps[shard].lock().unwrap());
    }

    fn into_maps(self) -> impl Iterator<Item = HashMap<Bytes, T>> {
        self.maps.into_iter().map(|map| map.into_inner().unwrap())
    }
}

fn scan<const N: usize>(paths: &[PathBuf], visit: impl Fn([&[u8]; N]) + Sync) -> Result<()> {
    paths.par_iter().try_for_each(|path| {
        let mut reader = BufReader::with_capacity(256 * 1024, fs::File::open(path)?);
        let mut line = Vec::new();
        reader.read_until(b'\n', &mut line)?;
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            let bytes = line.strip_suffix(b"\n").unwrap_or(&line);
            if let Some(values) = fields(bytes) {
                visit(values);
            }
        }
        Ok(())
    })
}

fn canonical<'a>(aliases: &'a HashMap<Bytes, Bytes>, id: &'a [u8]) -> &'a [u8] {
    aliases.get(id).map_or(id, |alias| alias.as_ref())
}

fn validate_event(
    values: [&[u8]; 7],
    ingest: i64,
    window: (i64, i64),
    aliases: &HashMap<Bytes, Bytes>,
    products: &HashMap<Bytes, u32>,
    unknown: u32,
) -> Option<(Bytes, Event)> {
    let [_, user, time, _, kind, product, price] = values;
    if user.is_empty() || product.is_empty() {
        return None;
    }
    let time = timestamp(time)?;
    if !(window.0..window.1).contains(&time) || ingest - time > DAY {
        return None;
    }
    let (revenue, category) = match kind {
        b"view" | b"cart" => (0, 0),
        b"purchase" => {
            let price = decimal(price)?;
            if !(1..=10_000_000).contains(&price) {
                return None;
            }
            (
                price as u32,
                products.get(product).copied().unwrap_or(unknown),
            )
        }
        _ => return None,
    };
    Some((
        canonical(aliases, user).into(),
        Event {
            time,
            revenue,
            category,
        },
    ))
}

fn summarize(
    events: &mut [Event],
    clicks: &mut Vec<Click>,
    organic: usize,
    campaign_days: &mut HashMap<(usize, i32), CampaignDay>,
) -> HashMap<i32, UserDay> {
    events.sort_unstable_by_key(|event| event.time);
    // Put the smallest ID last at a tied timestamp so the last eligible click wins.
    clicks.sort_unstable_by(|a, b| a.time.cmp(&b.time).then_with(|| b.id.cmp(&a.id)));
    clicks.dedup_by(|a, b| a.id == b.id);
    let mut user_days = HashMap::<i32, UserDay>::new();
    let mut active = HashMap::<usize, usize>::new();
    let (mut first, mut next) = (0, 0);
    let mut remaining: &[Event] = events;
    while let Some(start) = remaining.first().map(|event| event.time) {
        let day = start.div_euclid(DAY) as i32;
        let daily = user_days.entry(day).or_default();
        daily.sessions += 1;
        let mut previous = start;
        let mut revenue = 0_i64;
        let mut count = 0;
        for event in remaining.iter() {
            if event.time - previous > 1_800_000 {
                break;
            }
            previous = event.time;
            count += 1;
            if event.revenue != 0 {
                let price = i64::from(event.revenue);
                revenue += price;
                *daily.categories.entry(event.category).or_default() += price;
            }
        }
        remaining = &remaining[count..];
        daily.events += count as i64;
        daily.revenue += revenue;

        while next < clicks.len() && clicks[next].time <= start {
            *active.entry(clicks[next].campaign).or_default() += 1;
            next += 1;
        }
        while first < next && clicks[first].time < start - WEEK {
            let Entry::Occupied(mut count) = active.entry(clicks[first].campaign) else {
                unreachable!("each expired click was active");
            };
            *count.get_mut() -= 1;
            if *count.get() == 0 {
                count.remove();
            }
            first += 1;
        }
        let credited = if first < next {
            clicks[next - 1].campaign
        } else {
            organic
        };
        let totals = campaign_days.entry((credited, day)).or_default();
        totals.sessions += 1;
        totals.purchasing += i64::from(revenue > 0);
        totals.revenue += revenue;
        for &campaign in active.keys() {
            campaign_days.entry((campaign, day)).or_default().assists += 1;
        }
    }
    user_days
}

fn main() -> Result<()> {
    let started = Instant::now();
    let data = PathBuf::from(env::var_os("DATA_DIR").unwrap_or_else(|| "/data".into()));
    let destination = PathBuf::from(env::var_os("OUTPUT_DIR").unwrap_or_else(|| "/output".into()));
    if let Ok(cpus) = env::var("NCPU") {
        rayon::ThreadPoolBuilder::new()
            .num_threads(cpus.parse()?)
            .build_global()?;
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(data.join("manifest.json"))?)?;
    let bound = |key: &str| -> Result<i64> {
        manifest[key]
            .as_str()
            .and_then(|value| timestamp(value.as_bytes()))
            .ok_or_else(|| format!("invalid manifest field {key}").into())
    };
    let window = (bound("window_start")?, bound("window_end")?);

    let mut people = People::default();
    for path in files(&data.join("links"))? {
        for [left, right] in rows(&fs::read(path)?) {
            if !left.is_empty() && !right.is_empty() {
                people.join(left, right);
            }
        }
    }
    let aliases = people.finish();

    let products_raw = fs::read(data.join("products.csv"))?;
    let mut categories: Vec<&[u8]> = rows::<2>(&products_raw)
        .map(|[_, category]| category)
        .collect();
    categories.push(b"unknown");
    categories.sort_unstable();
    categories.dedup();
    let unknown = categories.binary_search(&b"unknown".as_slice()).unwrap() as u32;
    let products = rows(&products_raw)
        .map(|[id, category]| {
            (
                id.into(),
                categories.binary_search(&category).unwrap() as u32,
            )
        })
        .collect();
    let campaigns_raw = fs::read(data.join("campaigns.csv"))?;
    let mut campaigns: Vec<(&[u8], &[u8])> = rows(&campaigns_raw)
        .map(|[id, channel]| (id, channel))
        .collect();
    let organic = campaigns.len();
    campaigns.push((b"organic", b"organic"));
    let campaign_ids: HashMap<&[u8], usize> = campaigns[..organic]
        .iter()
        .enumerate()
        .map(|(index, &(name, _))| (name, index))
        .collect();

    let candidates = Shards::<Candidate>::new();
    scan(&files(&data.join("events"))?, |values: [&[u8]; 7]| {
        let id = values[0];
        if id.is_empty() {
            return;
        }
        let Some(ingest) = timestamp(values[3]) else {
            return;
        };
        // Invalid candidates remain in the map to suppress later valid copies.
        let candidate = Candidate {
            ingest,
            valid: validate_event(values, ingest, window, &aliases, &products, unknown),
        };
        candidates.update(id, |map| {
            if let Some(current) = map.get_mut(id) {
                if ingest < current.ingest {
                    *current = candidate;
                }
            } else {
                map.insert(id.into(), candidate);
            }
        });
    })?;
    eprintln!(
        "Events parsed and deduplicated in {:.2?}",
        started.elapsed()
    );

    let events = Shards::<Vec<Event>>::new();
    candidates
        .into_maps()
        .collect::<Vec<_>>()
        .into_par_iter()
        .for_each(|map| {
            for (user, event) in map.into_values().filter_map(|candidate| candidate.valid) {
                events.update(&user, |map| {
                    if let Some(records) = map.get_mut(user.as_ref()) {
                        records.push(event);
                    } else {
                        map.insert(user.clone(), vec![event]);
                    }
                });
            }
        });

    let clicks = Shards::<Vec<Click>> {
        hash: events.hash.clone(),
        ..Shards::new()
    };
    scan(
        &files(&data.join("clicks"))?,
        |[id, user, campaign, time]| {
            if id.is_empty() || user.is_empty() {
                return;
            }
            let Some(&campaign) = campaign_ids.get(campaign) else {
                return;
            };
            let Some(time) = timestamp(time) else {
                return;
            };
            if !(window.0 - WEEK..window.1).contains(&time) {
                return;
            }
            let user = canonical(&aliases, user);
            // A click ID's copies are identical. Collapse them during the sorted sweep.
            clicks.update(user, |map| {
                let click = Click {
                    time,
                    campaign,
                    id: id.into(),
                };
                if let Some(records) = map.get_mut(user) {
                    records.push(click);
                } else {
                    map.insert(user.into(), vec![click]);
                }
            });
        },
    )?;
    eprintln!("Person groups built in {:.2?}", started.elapsed());

    fs::create_dir_all(destination.join("user_daily"))?;
    let pairs: Vec<_> = events.into_maps().zip(clicks.into_maps()).collect();
    let totals: Vec<HashMap<(usize, i32), CampaignDay>> = pairs
        .into_par_iter()
        .enumerate()
        .map(|(index, (events, mut clicks))| -> Result<_> {
            let mut campaign_days = HashMap::new();
            if events.is_empty() && index != 0 {
                return Ok(campaign_days);
            }
            let mut writer = output::UserWriter::new(&destination, index)?;
            for (user, mut events) in events {
                let mut clicks = clicks.remove(&user).unwrap_or_default();
                let days = summarize(&mut events, &mut clicks, organic, &mut campaign_days);
                for (day, daily) in days {
                    let top = daily
                        .categories
                        .iter()
                        .max_by(|(a, x), (b, y)| x.cmp(y).then_with(|| b.cmp(a)))
                        .map(|(&category, _)| categories[category as usize]);
                    writer.push(&user, day, daily, top)?;
                }
            }
            writer.close()?;
            Ok(campaign_days)
        })
        .collect::<Result<_>>()?;

    let mut daily = HashMap::<(usize, i32), CampaignDay>::new();
    for totals in totals {
        for (key, values) in totals {
            let combined = daily.entry(key).or_default();
            combined.sessions += values.sessions;
            combined.purchasing += values.purchasing;
            combined.revenue += values.revenue;
            combined.assists += values.assists;
        }
    }
    output::write_campaigns(Path::new(&destination), &campaigns, daily)?;
    eprintln!("Completed in {:.2?}", started.elapsed());
    Ok(())
}
