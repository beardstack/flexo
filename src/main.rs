extern crate http;
extern crate rand;
extern crate flexo;
extern crate serde;

use std::str;
use curl::easy::{Easy, Easy2, Handler, WriteError};
use std::time::Duration;
use std::io::prelude::*;
use std::io;
use http::Uri;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::BufWriter;
use flexo::*;
use serde::Deserialize;
use crate::mirror_config::{MirrorSelectionMethod, MirrorConfig};
use std::cmp::Ordering;

mod mirror_config;


static DIRECTORY: &str = "/tmp/curl_ex_out/";
static JSON_URI: &str = "https://www.archlinux.org/mirrors/status/json/";
// integer values are easier to handle than float, since we don't have things like NaN. Hence, we just
// scale the float values from the JSON file in order to obtain integer values.
static SCORE_SCALE: u64 = 1_000_000_000_000_000;

#[derive(PartialEq, Eq, Hash, Clone, Debug)]
struct DownloadProvider {
    uri: Uri,
    mirror_results: MirrorResults,
    country: String,
}

impl Provider for DownloadProvider {
    type J = DownloadJob;

    fn new_job(&self, order: DownloadOrder) -> DownloadJob {
        let uri_string = format!("{}/{}", self.uri, order.filepath);
        let uri = uri_string.parse::<Uri>().unwrap();
        let provider = self.clone();
        DownloadJob {
            provider,
            uri,
            order
        }
    }

    fn identifier(&self) -> &Uri {
        &self.uri
    }

    fn score(&self) -> MirrorResults {
        self.mirror_results
    }
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug, Default)]
struct MirrorResults {
    namelookup_duration: Duration,
    connect_duration: Duration,
}

impl Ord for MirrorResults {
    fn cmp(&self, other: &Self) -> Ordering {
        self.connect_duration.cmp(&other.connect_duration)
    }
}

impl PartialOrd for MirrorResults {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug)]
pub enum DownloadJobError {
    CurlError(curl::Error),
    HttpFailureStatus(u32),
}

#[derive(Debug)]
struct DownloadJob {
    provider: DownloadProvider,
    uri: Uri,
    order: DownloadOrder,
}

impl Job for DownloadJob {
    type S = MirrorResults;
    type JS = FileState;
    type C = DownloadChannel;
    type O = DownloadOrder;
    type P = DownloadProvider;
    type E = DownloadJobError;
    type CS = DownloadChannelState;
    type PI = Uri;

    fn provider(&self) -> &DownloadProvider {
        &self.provider
    }

    fn order(&self) -> DownloadOrder {
        self.order.clone()
    }

    fn execute(self, mut channel: DownloadChannel) -> JobResult<DownloadJob> {
        let url = format!("{}", &self.uri);
        channel.handle.url(&url).unwrap();
        // Limit the speed to facilitate debugging.
        channel.handle.max_recv_speed(3_495_253 * 2).unwrap();
        channel.handle.low_speed_time(std::time::Duration::from_secs(8)).unwrap();
        channel.handle.low_speed_limit(524_288_000).unwrap();
        channel.handle.follow_location(true).unwrap();
        match channel.progress_indicator() {
            None => {},
            Some(start) => {
                channel.handle.resume_from(start).unwrap();
            }
        }
        match channel.handle.perform() {
            Ok(()) => {
                let response_code = channel.handle.response_code().unwrap();
                if response_code >= 200 && response_code < 300 {
                    println!("Success!");
                    JobResult::Complete(JobCompleted::new(channel, self.provider))
                } else {
                    let termination = JobTerminated {
                        channel,
                        error: DownloadJobError::HttpFailureStatus(response_code),
                    };
                    JobResult::Error(termination)
                }
            },
            Err(e) => {
                match channel.progress_indicator() {
                    Some(size) if size > 0 => {
                        JobResult::Partial(JobPartiallyCompleted::new(channel, size))
                    }
                    _ => {
                        let termination = JobTerminated {
                            channel,
                            error: DownloadJobError::CurlError(e),
                        };
                        JobResult::Error(termination)
                    }
                }
            }
        }
    }
}

#[derive(PartialEq, Eq, Hash, Clone, Debug)]
struct DownloadOrder {
    /// This path is relative to the given root directory.
    filepath: String,
}

impl Order for DownloadOrder {
    type J = DownloadJob;

    fn new_channel(self) -> DownloadChannel {
        DownloadChannel {
            handle: Easy2::new(DownloadState::new(self).unwrap()),
            state: DownloadChannelState::new(),
        }
    }
}

#[derive(PartialEq, Eq, Hash, Clone, Debug, Copy)]
struct DownloadChannelState {
    is_reset: bool,
}

impl DownloadChannelState {
    fn new() -> Self {
        Self {
            is_reset: false
        }
    }
}

impl ChannelState for DownloadChannelState {
    type J = DownloadJob;

    fn reset(&mut self) {
        self.is_reset = true;
    }
}

#[derive(Debug)]
struct FileState {
    buf_writer: BufWriter<File>,
    size_written: u64,
}

impl JobState for FileState {}

impl DownloadState {
    pub fn new(order: DownloadOrder) -> std::io::Result<Self> {
        let f = OpenOptions::new().create(true).append(true).open(DIRECTORY.to_owned() + &order.filepath)?;
        let size_written = f.metadata()?.len();
        let buf_writer = BufWriter::new(f);
        let job_state = JobStateItem {
            order,
            state: Some(FileState {
                buf_writer,
                size_written,
            }),
        };
        Ok(DownloadState { job_state })
    }

    pub fn reset(&mut self, order: DownloadOrder) -> std::io::Result<()> {
        if order != self.job_state.order {
            let c = DownloadState::new(order.clone())?;
            self.job_state.state = c.job_state.state;
            self.job_state.order = order;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct DownloadState {
    job_state: JobStateItem<DownloadJob>
}

impl JobState for DownloadState {}

impl Handler for DownloadState {
    fn write(&mut self, data: &[u8]) -> Result<usize, WriteError> {
        match self.job_state.state.iter_mut().next() {
            None => panic!("Expected the state to be initialized."),
            Some(file_state) => {
                file_state.size_written += data.len() as u64;
                match file_state.buf_writer.write(data) {
                    Ok(size) => {
                        Ok(size)
                    },
                    Err(e) => {
                        println!("Error while writing data: {:?}", e);
                        Err(WriteError::Pause)
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
struct DownloadChannel {
    handle: Easy2<DownloadState>,
    state: DownloadChannelState,
}

impl Channel for DownloadChannel {
    type J = DownloadJob;

    fn progress_indicator(&self) -> Option<u64> {
        let file_state = self.handle.get_ref().job_state.state.as_ref().unwrap();
        let size_written = file_state.size_written;
        if size_written > 0 {
            Some(size_written)
        } else {
            None
        }
    }

    fn reset_order(&mut self, order: DownloadOrder) {
        self.handle.get_mut().reset(order).unwrap();
    }

    fn channel_state_item(&mut self) -> &mut JobStateItem<DownloadJob> {
        &mut self.handle.get_mut().job_state
    }

    fn channel_state(&self) -> DownloadChannelState {
        self.state
    }

    fn channel_state_ref(&mut self) -> &mut DownloadChannelState {
        &mut self.state
    }
}

#[derive(Deserialize, Debug)]
struct MirrorListOption {
    pub urls: Vec<MirrorUrlOption>,
}

struct MirrorList {
    urls: Vec<MirrorUrl>,
}

impl From<MirrorListOption> for MirrorList {
    fn from(mirror_list_option: MirrorListOption) -> Self {
        let urls: Vec<Option<MirrorUrl>> = mirror_list_option.urls.into_iter().map(|mirror_url_option| {
            mirror_url_option.mirror_url()
        }).collect();
        let urls: Vec<MirrorUrl> = urls.into_iter().filter_map(|x| x).collect();
        MirrorList {
            urls
        }
    }
}

#[serde(rename_all = "lowercase")]
#[derive(Deserialize, Debug, PartialEq, Eq)]
pub enum MirrorProtocol {
    Http,
    Https,
    Rsync,
}

#[derive(Deserialize, Debug)]
pub struct MirrorUrlOption {
    pub url: String,
    pub protocol: Option<MirrorProtocol>,
    pub last_sync: Option<String>,
    pub completion_pct: Option<f64>,
    pub delay: Option<i32>,
    pub duration_avg: Option<f64>,
    pub duration_stddev: Option<f64>,
    pub score: Option<f64>,
    pub country: Option<String>,
    pub ipv4: Option<bool>,
    pub ipv6: Option<bool>,
}

impl MirrorUrlOption {
    pub fn mirror_url(self) -> Option<MirrorUrl> {
        let protocol = self.protocol?;
        let last_sync = self.last_sync?;
        let completion_pct = self.completion_pct?;
        let delay = self.delay?;
        let duration_avg = self.duration_avg?;
        let duration_stddev = self.duration_stddev?;
        let score = (self.score? * SCORE_SCALE as f64) as u64;
        let country = self.country?;
        let ipv4 = self.ipv4?;
        let ipv6 = self.ipv6?;
        Some(MirrorUrl {
            url: self.url,
            protocol,
            last_sync,
            completion_pct,
            delay,
            duration_avg,
            duration_stddev,
            score,
            country,
            ipv4,
            ipv6
        })
    }
}

#[derive(Debug)]
pub struct MirrorUrl {
    url: String,
    protocol: MirrorProtocol,
    last_sync: String,
    completion_pct: f64,
    delay: i32,
    duration_avg: f64,
    duration_stddev: f64,
    score: u64,
    country: String,
    ipv4: bool,
    ipv6: bool,
}

impl MirrorUrl {
    fn filter_predicate(&self, config: &MirrorConfig) -> bool {
        !(
            (config.mirrors_auto.https_required && self.protocol != MirrorProtocol::Https) ||
            (config.mirrors_auto.ipv4 && !self.ipv4) ||
            (config.mirrors_auto.ipv6 && !self.ipv6) ||
            (config.mirrors_auto.max_score < (self.score as f64) / (SCORE_SCALE as f64)) ||
            (config.mirrors_blacklist.contains(&self.url)))
    }
}

fn fetch_json() -> String {
    let mut received = Vec::new();
    let mut easy = Easy::new();
    easy.url(JSON_URI).unwrap();
    {
        let mut transfer = easy.transfer();
        transfer.write_function(|data| {
            received.extend_from_slice(data);
            Ok(data.len())
        }).unwrap();
        transfer.perform().unwrap();
    }
    std::str::from_utf8(received.as_slice()).unwrap().to_owned()
}

fn fetch_providers() -> Vec<MirrorUrl> {
    let json = fetch_json();
    let mirror_list_option: MirrorListOption = serde_json::from_str(&json).unwrap();
    let mirror_list: MirrorList = MirrorList::from(mirror_list_option);
    mirror_list.urls
}

fn measure_latency(url: &str, timeout: Duration) -> Option<MirrorResults> {
    let mut easy = Easy::new();
    easy.url(url).unwrap();
    easy.follow_location(true).unwrap();
    easy.connect_only(true).unwrap();
    easy.dns_cache_timeout(Duration::from_secs(3600 * 24)).unwrap();
    easy.connect_timeout(timeout).unwrap();
    easy.transfer().perform().ok()?;
    Some(MirrorResults {
        namelookup_duration: easy.namelookup_time().ok()?,
        connect_duration: easy.connect_time().ok()?,
    })
}

fn main() {
    let mirror_config = mirror_config::load_config();
    let providers: Vec<DownloadProvider> = if mirror_config.mirror_selection_method == MirrorSelectionMethod::Auto {
        let mut mirror_urls: Vec<MirrorUrl> = fetch_providers();
        mirror_urls.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap());
        let filtered_mirror_urls: Vec<MirrorUrl> = mirror_urls
            .into_iter()
            .filter(|x| x.filter_predicate(&mirror_config))
            .take(mirror_config.mirrors_auto.num_mirrors)
            .collect();
        let mut mirrors_with_latencies = Vec::new();
        let timeout = Duration::from_millis(mirror_config.mirrors_auto.timeout);
        for mirror in filtered_mirror_urls.into_iter() {
            match measure_latency(&mirror.url, timeout) {
                None => {},
                Some(latency) => {
                    mirrors_with_latencies.push((mirror, latency));
                }
            }
        }
        mirrors_with_latencies.sort_unstable_by_key(|(_, latency)| {
            *latency
        });

        mirrors_with_latencies.into_iter().map(|(mirror, mirror_results)| {
            DownloadProvider {
                uri: mirror.url.parse::<Uri>().unwrap(),
                mirror_results,
                country: mirror.country,
            }
        }).collect()
    } else {
        let default_mirror_result: MirrorResults = Default::default();
        mirror_config.mirrors_predefined.into_iter().map(|uri| {
            DownloadProvider {
                uri: uri.parse::<Uri>().unwrap(),
                mirror_results: default_mirror_result,
                country: "Unknown".to_owned(),
            }
        }).collect()
    };
    println!("{:#?}", providers);
    let mut job_context: JobContext<DownloadJob> = JobContext::new(providers);

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let filename: String = line.unwrap();
        let order = DownloadOrder {
            filepath: filename,
        };
        job_context.schedule(order);
    }
}
