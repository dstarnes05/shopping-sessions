#[cfg(target_arch = "x86_64")]
mod block;
mod ids;
use ahash::AHashMap;
mod input;
mod output;

use std::{
    collections::HashMap,
    env,
    error::Error,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

use input::{decimal, files, rows, scan_chunks, timestamp, DAY, WEEK};
use rayon::prelude::*;

type Bytes = Box<[u8]>;
type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Default)]
struct People {
    ids: AHashMap<u64, usize>,
    names: Vec<u64>,
    parents: Vec<usize>,
    sizes: Vec<usize>,
    minimum: Vec<usize>,
}

impl People {
    fn intern(&mut self, name: u64) -> usize {
        if let Some(&id) = self.ids.get(&name) {
            return id;
        }
        let id = self.names.len();
        self.ids.insert(name, id);
        self.names.push(name);
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

    fn join(&mut self, left: u64, right: u64, users: &ids::Ids) {
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
        if users
            .compare(
                self.names[self.minimum[right]],
                self.names[self.minimum[left]],
            )
            .is_lt()
        {
            self.minimum[left] = self.minimum[right];
        }
    }

    fn finish(mut self) -> AHashMap<u64, u64> {
        let canonical: Vec<_> = (0..self.names.len())
            .map(|id| {
                let root = self.root(id);
                self.minimum[root]
            })
            .collect();
        self.ids
            .into_iter()
            .filter_map(|(name, id)| {
                (id != canonical[id]).then(|| (name, self.names[canonical[id]]))
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
    id: u64,
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

#[derive(Clone, Copy)]
struct Candidate {
    id: u64,
    ingest: i64,
    user: u64,
    event: Event,
}

mod dedup;

const SHARDS: usize = 256;
const PARTITIONS_PER_FILE: usize = 8;

fn shard(id: u64) -> usize {
    (id.wrapping_mul(0x9e3779b97f4a7c15) >> 56) as usize
}

fn partitions<T>() -> Vec<Mutex<Vec<T>>> {
    (0..SHARDS).map(|_| Mutex::new(Vec::new())).collect()
}

fn flush<T>(batches: Vec<Vec<T>>, partitions: &[Mutex<Vec<T>>]) {
    for (mut batch, destination) in batches.into_iter().zip(partitions) {
        if !batch.is_empty() {
            destination.lock().unwrap().append(&mut batch);
        }
    }
}

fn validate_event(
    dates: &mut input::Dates,
    values: [&[u8]; 7],
    ingest: i64,
    window: (i64, i64),
    products: &HashMap<Bytes, u32>,
    unknown: u32,
) -> Option<Event> {
    let [_, user, time, _, kind, product, price] = values;
    if user.is_empty() || product.is_empty() {
        return None;
    }
    let time = dates.parse(time)?;
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
    Some(Event {
        time,
        revenue,
        category,
    })
}

// Reused for each person's sliding click window. The dense list allows
// assist credit to visit just the campaigns present in that window.
struct ActiveCampaigns {
    counts: Vec<usize>,
    positions: Vec<usize>,
    ids: Vec<usize>,
}

impl ActiveCampaigns {
    fn new(campaigns: usize) -> Self {
        Self {
            counts: vec![0; campaigns],
            positions: vec![0; campaigns],
            ids: Vec::new(),
        }
    }
    fn add(&mut self, id: usize) {
        if self.counts[id] == 0 {
            self.positions[id] = self.ids.len();
            self.ids.push(id);
        }
        self.counts[id] += 1;
    }
    fn remove(&mut self, id: usize) {
        self.counts[id] -= 1;
        if self.counts[id] == 0 {
            let index = self.positions[id];
            self.ids.swap_remove(index);
            if let Some(&moved) = self.ids.get(index) {
                self.positions[moved] = index;
            }
        }
    }
    fn clear(&mut self) {
        for id in self.ids.drain(..) {
            self.counts[id] = 0;
        }
    }
}

fn summarize(
    events: &mut [Event],
    clicks: &[(u64, Click)],
    organic: usize,
    active: &mut ActiveCampaigns,
    campaign_days: &mut AHashMap<i32, Vec<CampaignDay>>,
) -> Vec<(i32, UserDay)> {
    events.sort_unstable_by_key(|event| event.time);
    let mut user_days = Vec::<(i32, UserDay)>::new();
    active.clear();
    let (mut first, mut next) = (0, 0);
    let mut remaining: &[Event] = events;
    while let Some(start) = remaining.first().map(|event| event.time) {
        let day = start.div_euclid(DAY) as i32;
        if user_days
            .last()
            .is_none_or(|(previous, _)| *previous != day)
        {
            user_days.push((day, UserDay::default()));
        }
        let daily = &mut user_days.last_mut().unwrap().1;
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

        while next < clicks.len() && clicks[next].1.time <= start {
            active.add(clicks[next].1.campaign);
            next += 1;
        }
        while first < next && clicks[first].1.time < start - WEEK {
            active.remove(clicks[first].1.campaign);
            first += 1;
        }
        let credited = if first < next {
            clicks[next - 1].1.campaign
        } else {
            organic
        };
        let campaigns = campaign_days
            .entry(day)
            .or_insert_with(|| vec![CampaignDay::default(); organic + 1]);
        let totals = &mut campaigns[credited];
        totals.sessions += 1;
        totals.purchasing += i64::from(revenue > 0);
        totals.revenue += revenue;
        for &campaign in &active.ids {
            campaigns[campaign].assists += 1;
        }
    }
    user_days
}

fn load_aliases(data: &std::path::Path, users: &ids::Ids) -> Result<AHashMap<u64, u64>> {
    let mut people = People::default();
    for path in files(&data.join("links"))? {
        for [left, right] in rows(&fs::read(path)?) {
            if !left.is_empty() && !right.is_empty() {
                people.join(users.encode(left), users.encode(right), users);
            }
        }
    }
    Ok(people.finish())
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

    let users = ids::Ids::new(b'u');
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

    let event_ids = ids::Ids::new(b'e');
    let candidates: Vec<Mutex<Vec<Vec<Candidate>>>> = partitions();
    let ingest_started = Instant::now();
    let event_paths = files(&data.join("events"))?;
    let (aliases, scanned) = rayon::join(
        || load_aliases(&data, &users),
        || {
            scan_chunks(&event_paths, |chunk| {
                let mut batches = vec![Vec::new(); SHARDS];
                let mut dates = input::Dates::default();
                input::visit_rows::<7>(chunk, |values| {
                    if values[0].is_empty() {
                        return;
                    }
                    let Some(ingest) = dates.parse(values[3]) else {
                        return;
                    };
                    let id = event_ids.encode(values[0]);
                    // Invalid winners suppress later copies. No accepted timestamp can
                    // equal i64::MIN under the supported timestamp grammar.
                    let (user, event) = match validate_event(
                        &mut dates, values, ingest, window, &products, unknown,
                    ) {
                        Some(event) => (users.encode(values[1]), event),
                        None => (
                            0,
                            Event {
                                time: i64::MIN,
                                revenue: 0,
                                category: 0,
                            },
                        ),
                    };
                    batches[shard(id)].push(Candidate {
                        id,
                        ingest,
                        user,
                        event,
                    });
                });
                for (batch, partition) in batches.into_iter().zip(&candidates) {
                    if !batch.is_empty() {
                        partition.lock().unwrap().push(batch);
                    }
                }
            })
        },
    );
    scanned?;
    let aliases = aliases?;
    let canonical = |id| aliases.get(&id).copied().unwrap_or(id);
    eprintln!("Event scan: {:.2?}", ingest_started.elapsed());
    drop(event_ids);

    let grouping_started = Instant::now();
    let events = partitions();
    candidates.into_par_iter().for_each(|partition| {
        let chunks = partition.into_inner().unwrap();
        let mut batches = vec![Vec::new(); SHARDS];
        dedup::winners(chunks, |winner| {
            if winner.event.time != i64::MIN {
                let user = canonical(winner.user);
                batches[shard(user)].push((user, winner.event));
            }
        });
        flush(batches, &events);
    });
    eprintln!(
        "Deduplication and event grouping: {:.2?}",
        grouping_started.elapsed()
    );

    let click_ids = ids::Ids::new(b'k');
    let clicks = partitions();
    scan_chunks(&files(&data.join("clicks"))?, |chunk| {
        let mut batches: Vec<Vec<_>> = (0..SHARDS).map(|_| Vec::new()).collect();
        let mut dates = input::Dates::default();
        input::visit_rows::<4>(chunk, |[id, user, campaign, time]| {
            if id.is_empty() || user.is_empty() {
                return;
            }
            let Some(&campaign) = campaign_ids.get(campaign) else {
                return;
            };
            let Some(time) = dates.parse(time) else {
                return;
            };
            if !(window.0 - WEEK..window.1).contains(&time) {
                return;
            }
            let user = canonical(users.encode(user));
            batches[shard(user)].push((
                user,
                Click {
                    id: click_ids.encode(id),
                    time,
                    campaign,
                },
            ));
        });
        flush(batches, &clicks);
    })?;
    let output_started = Instant::now();

    fs::create_dir_all(destination.join("user_daily"))?;
    let pairs: Vec<_> = events
        .into_iter()
        .zip(clicks)
        .map(|(events, clicks)| (events.into_inner().unwrap(), clicks.into_inner().unwrap()))
        .collect();
    let profile = env::var_os("PROFILE_OUTPUT").is_some();
    let session_us = AtomicU64::new(0);
    let row_output_us = AtomicU64::new(0);
    let parquet_us = AtomicU64::new(0);
    let totals: Vec<AHashMap<i32, Vec<CampaignDay>>> = pairs
        .into_par_iter()
        .chunks(PARTITIONS_PER_FILE)
        .enumerate()
        .map(|(index, partitions)| -> Result<_> {
            let profile_started = profile.then(Instant::now);
            let mut writing = Duration::ZERO;
            let mut campaign_days = AHashMap::new();
            let mut active = ActiveCampaigns::new(organic);
            if index != 0 && partitions.iter().all(|(events, _)| events.is_empty()) {
                return Ok(campaign_days);
            }
            let mut writer = output::UserWriter::new(&destination, index)?;
            if let Some(start) = profile_started {
                writing += start.elapsed();
            }
            for (mut events, mut clicks) in partitions {
                clicks.sort_unstable_by(|(u, a), (v, b)| {
                    u.cmp(v)
                        .then_with(|| a.time.cmp(&b.time))
                        .then_with(|| click_ids.compare(b.id, a.id))
                });
                clicks.dedup_by(|a, b| a.1.id == b.1.id);
                let mut click_groups = clicks.chunk_by(|a, b| a.0 == b.0).peekable();
                events.sort_unstable_by_key(|record| record.0);
                let mut person_events = Vec::new();
                for group in events.chunk_by(|a, b| a.0 == b.0) {
                    person_events.clear();
                    person_events.extend(group.iter().map(|row| row.1));
                    let user_id = group[0].0;
                    let user = users.decode(user_id);
                    while click_groups.next_if(|g| g[0].0 < user_id).is_some() {}
                    let clicks = click_groups
                        .next_if(|g| g[0].0 == user_id)
                        .unwrap_or_default();
                    let days = summarize(
                        &mut person_events,
                        clicks,
                        organic,
                        &mut active,
                        &mut campaign_days,
                    );
                    let write_started = profile.then(Instant::now);
                    for (day, daily) in days {
                        let top = daily
                            .categories
                            .iter()
                            .max_by(|(a, x), (b, y)| x.cmp(y).then_with(|| b.cmp(a)))
                            .map(|(&category, _)| categories[category as usize]);
                        writer.push(&user, day, daily, top)?;
                    }
                    if let Some(start) = write_started {
                        writing += start.elapsed();
                    }
                }
            }
            let close_started = profile.then(Instant::now);
            let parquet_time = writer.close()?;
            if let (Some(start), Some(close)) = (profile_started, close_started) {
                writing += close.elapsed();
                session_us.fetch_add(
                    (start.elapsed() - writing).as_micros() as u64,
                    Ordering::Relaxed,
                );
                row_output_us.fetch_add(writing.as_micros() as u64, Ordering::Relaxed);
                parquet_us.fetch_add(parquet_time.as_micros() as u64, Ordering::Relaxed);
            }
            Ok(campaign_days)
        })
        .collect::<Result<_>>()?;

    let mut daily = HashMap::<(usize, i32), CampaignDay>::new();
    for totals in totals {
        for (day, campaigns) in totals {
            for (campaign, values) in campaigns.into_iter().enumerate() {
                if values.sessions == 0 && values.assists == 0 {
                    continue;
                }
                let combined = daily.entry((campaign, day)).or_default();
                combined.sessions += values.sessions;
                combined.purchasing += values.purchasing;
                combined.revenue += values.revenue;
                combined.assists += values.assists;
            }
        }
    }
    let campaign_started = Instant::now();
    output::write_campaigns(&destination, &campaigns, daily)?;
    if profile {
        eprintln!("Person grouping and session aggregation: {:.3} worker-s; user row preparation: {:.3} worker-s; user Parquet encode/write: {:.3} worker-s; campaign Parquet: {:.2?} elapsed",
            session_us.load(Ordering::Relaxed) as f64 / 1e6,
            (row_output_us.load(Ordering::Relaxed) - parquet_us.load(Ordering::Relaxed)) as f64 / 1e6,
            parquet_us.load(Ordering::Relaxed) as f64 / 1e6, campaign_started.elapsed());
    }
    eprintln!(
        "Sessions, attribution and Parquet output: {:.2?}",
        output_started.elapsed()
    );
    eprintln!("Completed in {:.2?}", started.elapsed());
    Ok(())
}
