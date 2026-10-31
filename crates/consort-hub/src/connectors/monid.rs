//! A small client for the Monid API, with only the calls that the hub needs
//! (ADR 0016).
//!
//! Monid publishes no OpenAPI file that matches its API. The paths and the
//! shapes come from its CLI (`@monid-ai/cli` 0.1.8) and from real calls on
//! 2026-10-31. Every request sends the key as a bearer token.
//!
//! The key is never logged: [`ApiKey`] has a redacted `Debug`, and errors
//! never hold a request header.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use consort_proto::error::ErrorCode;
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Value, json};

use super::composio::ApiKey;
use crate::error::{HubError, Result};

/// The API base. The CLI reads the same value.
pub(crate) const DEFAULT_BASE_URL: &str = "https://api.monid.ai";
/// The longest wait for one request. A run is polled, so a request is short.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// The largest answer that the hub reads. The output of a run can be large.
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
/// The most results of one search.
pub(crate) const SEARCH_LIMIT_MAX: u32 = 25;
/// How many millionths of a dollar are in one dollar.
const MICROS_PER_DOLLAR: f64 = 1_000_000.0;

/// An amount of money in Monid's answers.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Amount {
    pub value: f64,
}

impl Amount {
    /// The amount in millionths of a dollar. Hub totals add whole numbers,
    /// so a long sum does not drift. A part of a millionth rounds up.
    pub(crate) fn micros(self) -> i64 {
        dollars_to_micros(self.value)
    }
}

/// Dollars to millionths of a dollar. A part of a millionth rounds up.
pub(crate) fn dollars_to_micros(dollars: f64) -> i64 {
    if !dollars.is_finite() || dollars <= 0.0 {
        return 0;
    }
    // Rounded first, because 0.0035 * 1e6 is not exactly 3500 in binary.
    let rounded = (dollars * MICROS_PER_DOLLAR * 1_000.0).round() / 1_000.0;
    rounded.ceil() as i64
}

/// A tool in a search result, and the answer of `POST /v1/inspect`. Only
/// inspect has `input`, `metrics` and `categories`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Tool {
    pub provider: String,
    pub provider_name: String,
    pub endpoint: String,
    pub description: String,
    pub summary: String,
    pub price: Value,
    pub input: Value,
    pub tags: Vec<String>,
    pub categories: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct SearchAnswer {
    results: Vec<Tool>,
}

/// The state of a run. The API gives upper-case names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunStatus {
    /// `READY` or `RUNNING`, and any state that this hub does not know yet.
    Going,
    Completed,
    Failed,
    /// A budget or a run cap on Monid's site stopped the run.
    Blocked,
    Stopped,
    TimedOut,
}

impl RunStatus {
    fn parse(text: &str) -> Self {
        match text {
            "COMPLETED" => Self::Completed,
            "FAILED" => Self::Failed,
            "BLOCKED" => Self::Blocked,
            "STOPPED" => Self::Stopped,
            "TIME_OUT" => Self::TimedOut,
            _ => Self::Going,
        }
    }

    pub(crate) fn is_final(self) -> bool {
        self != Self::Going
    }
}

/// `GET /v1/runs/{id}`. `POST /v1/run` gives a part of it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Run {
    pub run_id: String,
    status: String,
    pub cost: Option<Amount>,
    pub output: Value,
    pub provider_response: ProviderResponse,
    pub stoppable: bool,
    pub error: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ProviderResponse {
    pub http_status: Option<u16>,
}

impl Run {
    pub(crate) fn status(&self) -> RunStatus {
        RunStatus::parse(&self.status)
    }

    /// What Monid charged, in millionths of a dollar. `cost.value` of a run
    /// is the charge: the balance fell by exactly that in a real call.
    pub(crate) fn cost_micros(&self) -> i64 {
        self.cost.map(Amount::micros).unwrap_or(0)
    }

    /// True when the run finished but the provider answered with an error.
    /// A real provider error was a `COMPLETED` run with no charge.
    pub(crate) fn provider_failed(&self) -> bool {
        self.provider_response
            .http_status
            .is_some_and(|status| status >= 400)
    }
}

/// `GET /v1/wallet/balance`.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Balance {
    pub balance: Amount,
    pub held: Amount,
}

fn monid_error(message: impl Into<String>) -> HubError {
    HubError::new(ErrorCode::MonidError, message)
}

/// Whether Monid refused the key.
pub(crate) fn is_unauthorized(err: &HubError) -> bool {
    err.code() == ErrorCode::MonidKeyRefused
}

/// The start of what a 402 from Monid says.
const LOW_BALANCE_TEXT: &str = "The Monid balance is too low.";

/// Whether Monid refused a run because the balance is too low.
pub(crate) fn is_low_balance(err: &HubError) -> bool {
    err.to_error_body().message.starts_with(LOW_BALANCE_TEXT)
}

/// A client for one key.
#[derive(Clone)]
pub(crate) struct Client {
    http: reqwest::Client,
    base: String,
    key: ApiKey,
}

impl Client {
    pub(crate) fn new(base: &str, key: ApiKey) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("consort-hub/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| HubError::internal("the HTTP client did not start"))?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            key,
        })
    }

    async fn send(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let url = format!("{}{path}", self.base);
        let mut request = self
            .http
            .request(method, &url)
            .bearer_auth(self.key.expose());
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(|err| {
            if err.is_timeout() {
                return monid_error("Monid did not answer in time. Try again in a moment.");
            }
            monid_error(format!(
                "The hub could not reach Monid: {}. Check that the hub can reach the internet, then try again.",
                err.without_url()
            ))
        })?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|len| len > MAX_RESPONSE_BYTES as u64)
        {
            return Err(monid_error("Monid sent an answer that is too large."));
        }
        let bytes = response.bytes().await.map_err(|err| {
            monid_error(format!(
                "The answer from Monid stopped: {}. Try again.",
                err.without_url()
            ))
        })?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(monid_error("Monid sent an answer that is too large."));
        }
        let value: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        if status.is_success() {
            return Ok(value);
        }
        Err(status_error(status.as_u16(), &value))
    }

    /// `POST /v1/discover`. A search costs nothing.
    pub(crate) async fn search(&self, query: &str, limit: u32) -> Result<Vec<Tool>> {
        let body = json!({"query": query, "limit": limit.clamp(1, SEARCH_LIMIT_MAX)});
        let value = self.send(Method::POST, "/v1/discover", Some(&body)).await?;
        let answer: SearchAnswer = serde_json::from_value(value)
            .map_err(|_| monid_error("Monid sent a search result that the hub cannot read."))?;
        Ok(answer.results)
    }

    /// `POST /v1/inspect`. It costs nothing.
    pub(crate) async fn inspect(&self, provider: &str, endpoint: &str) -> Result<Tool> {
        let body = json!({"provider": provider, "endpoint": endpoint});
        let value = self.send(Method::POST, "/v1/inspect", Some(&body)).await?;
        serde_json::from_value(value)
            .map_err(|_| monid_error("Monid sent a tool that the hub cannot read."))
    }

    /// `POST /v1/run`. Bad input gets an error with no run and no charge.
    /// The hub never sends this twice for one call: Monid documents no
    /// idempotency, so a second try can charge twice.
    pub(crate) async fn start_run(
        &self,
        provider: &str,
        endpoint: &str,
        input: &Value,
    ) -> Result<Run> {
        let mut body = json!({"provider": provider, "endpoint": endpoint});
        if input.as_object().is_some_and(|map| !map.is_empty()) {
            body["input"] = input.clone();
        }
        let value = self.send(Method::POST, "/v1/run", Some(&body)).await?;
        serde_json::from_value(value)
            .map_err(|_| monid_error("Monid sent a run that the hub cannot read."))
    }

    /// `GET /v1/runs/{id}`.
    pub(crate) async fn run(&self, run_id: &str) -> Result<Run> {
        let path = format!("/v1/runs/{}", path_part(run_id));
        let value = self.send(Method::GET, &path, None).await?;
        serde_json::from_value(value)
            .map_err(|_| monid_error("Monid sent a run that the hub cannot read."))
    }

    /// `POST /v1/runs/{id}/stop`. Monid accepts it for a stoppable run only.
    pub(crate) async fn stop_run(&self, run_id: &str) -> Result<()> {
        let path = format!("/v1/runs/{}/stop", path_part(run_id));
        self.send(Method::POST, &path, None).await?;
        Ok(())
    }

    /// `GET /v1/wallet/balance`. It also checks the key.
    pub(crate) async fn balance(&self) -> Result<Balance> {
        let value = self.send(Method::GET, "/v1/wallet/balance", None).await?;
        serde_json::from_value(value)
            .map_err(|_| monid_error("Monid sent a balance that the hub cannot read."))
    }
}

/// A value for one part of a URL path. A run ID has only letters and digits,
/// so anything else is left out.
fn path_part(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect()
}

fn status_error(status: u16, value: &Value) -> HubError {
    if matches!(status, 401 | 403) {
        return HubError::new(
            ErrorCode::MonidKeyRefused,
            "Monid refuses the key. Make a new key at app.monid.ai, then add it in Settings > Connectors.",
        );
    }
    let detail = ["message", "error"]
        .iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .or_else(|| {
            value
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
        })
        .map(|text| crate::util::truncate_chars(text.trim(), 1_000));
    match (status, detail) {
        (400 | 422, Some(detail)) => HubError::invalid(format!(
            "Monid did not accept the input, so nothing ran and nothing was charged: {detail}"
        )),
        (400 | 422, None) => HubError::invalid(
            "Monid did not accept the input, so nothing ran and nothing was charged.",
        ),
        (402, _) => monid_error(format!(
            "{LOW_BALANCE_TEXT} Ask the user to add money at app.monid.ai/wallet."
        )),
        (404, _) => HubError::invalid(
            "Monid has no such tool. Call search_tools to find one, then get_monid_tool.",
        ),
        (429, _) => monid_error("Monid asks the hub to slow down. Try again in a moment."),
        (_, Some(detail)) => monid_error(format!("Monid answered with an error: {detail}")),
        (_, None) => monid_error(format!("Monid answered with status {status}.")),
    }
}

/// The largest file that the hub takes from a run.
const FILE_MAX_BYTES: usize = 64 * 1024 * 1024;
/// How many redirects a file download follows.
const FILE_REDIRECTS_MAX: usize = 3;
/// The longest wait for one file.
const FILE_TIMEOUT: Duration = Duration::from_secs(120);

/// A file that the hub downloaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Downloaded {
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// Whether an address is on the public internet. A run's output comes from a
/// third party, so the hub must not fetch its own network for it.
pub(crate) fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                || a >= 224)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || (first & 0xff00) == 0xff00)
        }
    }
}

/// Downloads a file from a run's output. Each hop resolves its host, refuses
/// an address that is not public, and connects only to the addresses that it
/// checked. `allow_private` is for a local fake server in tests.
pub(crate) async fn download_file(url: &str, allow_private: bool) -> Result<Downloaded> {
    let failed = |why: &str| monid_error(format!("The hub could not download the file: {why}"));
    let mut current = reqwest::Url::parse(url).map_err(|_| failed("its address is not valid."))?;
    for _ in 0..=FILE_REDIRECTS_MAX {
        if !matches!(current.scheme(), "http" | "https") {
            return Err(failed("its address is not http or https."));
        }
        let host = current
            .host_str()
            .ok_or_else(|| failed("its address has no host."))?
            .to_owned();
        let port = current
            .port_or_known_default()
            .ok_or_else(|| failed("its address has no port."))?;
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|_| failed("its host cannot be found."))?
            .collect();
        if addrs.is_empty() || (!allow_private && addrs.iter().any(|addr| !is_public_ip(addr.ip())))
        {
            return Err(failed("its host is not on the public internet."));
        }
        let http = reqwest::Client::builder()
            .timeout(FILE_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(&host, &addrs)
            .user_agent(concat!("consort-hub/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| HubError::internal("the HTTP client did not start"))?;
        let response = http
            .get(current.clone())
            .send()
            .await
            .map_err(|err| failed(&err.without_url().to_string()))?;
        let status = response.status();
        if status.is_redirection() {
            let next = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|location| current.join(location).ok())
                .ok_or_else(|| failed("it redirects to an address that is not valid."))?;
            current = next;
            continue;
        }
        if !status.is_success() {
            return Err(failed(&format!("the server answered {}.", status.as_u16())));
        }
        if response
            .content_length()
            .is_some_and(|len| len > FILE_MAX_BYTES as u64)
        {
            return Err(failed("it is larger than 64 MB."));
        }
        let mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.split(';').next().unwrap_or(value).trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "application/octet-stream".to_owned());
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| failed(&err.without_url().to_string()))?
        {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > FILE_MAX_BYTES {
                return Err(failed("it is larger than 64 MB."));
            }
        }
        return Ok(Downloaded { bytes, mime });
    }
    Err(failed("it redirects too many times."))
}

/// What a card says about the price of one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PriceQuote {
    /// The rate: "$0.0016 per result".
    pub rate: String,
    /// The estimate for this input, in millionths of a dollar. `None` when
    /// the input does not fix the cost, or the price reads the output.
    pub estimate_micros: Option<i64>,
}

/// Input fields that cap the number of results.
const LIMIT_FIELDS: &[&str] = &[
    "maxItems",
    "maxResults",
    "resultsLimit",
    "resultsPerPage",
    "max_results",
    "numResults",
    "limit",
    "count",
    "num",
    "n",
];
/// Input fields that hold the text of a per-character price.
const TEXT_FIELDS: &[&str] = &["text", "input", "prompt"];

/// The price of one call with this input. `input` has the shape of a run:
/// `{body, queryParams, pathParams}`. The real prices have the types
/// `PER_CALL`, `PER_RESULT`, `PER_UNIT`, `TIERED` and `PER_UNIT_MATRIX`.
pub(crate) fn quote(price: &Value, input: &Value) -> PriceQuote {
    let unknown = || PriceQuote {
        rate: "The price depends on the result".to_owned(),
        estimate_micros: None,
    };
    let amount = price
        .get("amount")
        .and_then(|a| a.get("value"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    match price.get("type").and_then(Value::as_str).unwrap_or("") {
        "PER_CALL" => PriceQuote {
            rate: format!("{} per call", money_text(dollars_to_micros(amount))),
            estimate_micros: Some(dollars_to_micros(amount)),
        },
        "PER_RESULT" => PriceQuote {
            rate: format!("{} per result", money_text(dollars_to_micros(amount))),
            estimate_micros: result_cap(input)
                .map(|cap| dollars_to_micros(amount).saturating_mul(cap)),
        },
        "PER_UNIT" => per_unit_quote(price, amount, input),
        "PER_UNIT_MATRIX" => {
            let variants = price.get("variants").and_then(Value::as_array);
            match variants.and_then(|list| matching_arm(list, input)) {
                Some(arm) => quote(arm.get("price").unwrap_or(&Value::Null), input),
                None => PriceQuote {
                    rate: format!("From {}", money_text(dollars_to_micros(amount))),
                    estimate_micros: None,
                },
            }
        }
        "TIERED" => {
            let tiers = price.get("tiers").and_then(Value::as_array);
            let reads_output = tiers.is_some_and(|list| {
                list.iter().any(|tier| {
                    tier.get("selector")
                        .and_then(|s| s.get("in"))
                        .and_then(Value::as_str)
                        == Some("output")
                })
            });
            let chosen = tiers
                .and_then(|list| matching_arm(list, input))
                .and_then(|arm| arm.get("price"))
                .or_else(|| price.get("default"));
            match chosen {
                Some(chosen) => {
                    let mut chosen = quote(chosen, input);
                    // More tiers can add to the price after the run.
                    if reads_output {
                        chosen.estimate_micros = None;
                    }
                    chosen
                }
                None => unknown(),
            }
        }
        _ => unknown(),
    }
}

fn per_unit_quote(price: &Value, amount: f64, input: &Value) -> PriceQuote {
    let per = price
        .get("per")
        .and_then(Value::as_f64)
        .filter(|p| *p > 0.0)
        .unwrap_or(1.0);
    let unit = price
        .get("unit")
        .and_then(Value::as_str)
        .unwrap_or("unit")
        .trim();
    let rate = if per == 1.0 {
        format!("{} per {unit}", money_text(dollars_to_micros(amount)))
    } else {
        format!(
            "{} per {} {unit}s",
            money_text(dollars_to_micros(amount)),
            whole_number_text(per as u64)
        )
    };
    let estimate_micros = (unit == "character")
        .then(|| {
            TEXT_FIELDS
                .iter()
                .find_map(|key| input_field(input, key).and_then(Value::as_str))
        })
        .flatten()
        .map(|text| dollars_to_micros(amount * text.chars().count() as f64 / per));
    PriceQuote {
        rate,
        estimate_micros,
    }
}

/// The first variant or tier whose `when` matches the input.
fn matching_arm<'a>(arms: &'a [Value], input: &Value) -> Option<&'a Value> {
    arms.iter().find(|arm| {
        arm.get("when")
            .and_then(Value::as_object)
            .is_some_and(|when| {
                !when.is_empty()
                    && when
                        .iter()
                        .all(|(key, wanted)| input_field(input, key) == Some(wanted))
            })
    })
}

/// A field of the input, from the body or the query.
fn input_field<'a>(input: &'a Value, key: &str) -> Option<&'a Value> {
    ["body", "queryParams"]
        .iter()
        .find_map(|part| input.get(part).and_then(|p| p.get(key)))
}

/// The most results that a per-result call can return, when the input caps
/// it. A cap often applies per search term, so it multiplies by the longest
/// list in the input.
fn result_cap(input: &Value) -> Option<i64> {
    let cap = LIMIT_FIELDS
        .iter()
        .find_map(|key| input_field(input, key).and_then(Value::as_i64))
        .filter(|cap| *cap > 0)?;
    let longest = ["body", "queryParams"]
        .iter()
        .filter_map(|part| input.get(part).and_then(Value::as_object))
        .flat_map(|map| map.values())
        .filter_map(Value::as_array)
        .map(|list| list.len() as i64)
        .max()
        .unwrap_or(1)
        .max(1);
    Some(cap.saturating_mul(longest))
}

/// Millionths of a dollar as text: "$0.0035", "$1.24", "$0".
pub(crate) fn money_text(micros: i64) -> String {
    let dollars = micros as f64 / MICROS_PER_DOLLAR;
    if micros == 0 {
        return "$0".to_owned();
    }
    if micros % 10_000 == 0 {
        return format!("${dollars:.2}");
    }
    let text = format!("{dollars:.6}");
    format!("${}", text.trim_end_matches('0'))
}

fn whole_number_text(number: u64) -> String {
    let digits = number.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// One file in the output of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutputFile {
    pub url: String,
    pub name: String,
}

/// The most files that the hub takes from one run.
const OUTPUT_FILES_MAX: usize = 8;
/// Output keys that hold a generated file. A scraper's result has many other
/// addresses, such as a profile picture on another site. The hub does not
/// fetch those: an address in scraped text can point anywhere.
const FILE_KEYS: &[&str] = &[
    "image_urls",
    "image_url",
    "images",
    "audio",
    "audio_url",
    "video",
    "video_url",
    "file",
    "file_url",
    "files",
];
/// How deep the file keys can be. A real image was at `data.image_urls`.
const FILE_KEY_DEPTH: usize = 3;

/// The generated files in the output of a run. A file output differs per
/// tool, and its address expires, so the hub downloads each one when the run
/// ends.
pub(crate) fn output_files(output: &Value) -> Vec<OutputFile> {
    let mut urls = Vec::new();
    collect_file_urls(output, 0, &mut urls);
    urls.dedup();
    urls.truncate(OUTPUT_FILES_MAX);
    urls.into_iter()
        .enumerate()
        .map(|(i, url)| OutputFile {
            name: file_name(&url, i),
            url,
        })
        .collect()
}

fn collect_file_urls(value: &Value, depth: usize, out: &mut Vec<String>) {
    let Some(map) = value.as_object() else {
        return;
    };
    for (key, child) in map {
        if FILE_KEYS.contains(&key.as_str()) {
            push_urls(child, out);
        } else if depth < FILE_KEY_DEPTH {
            collect_file_urls(child, depth + 1, out);
        }
    }
}

fn push_urls(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) if is_http(text) => out.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| push_urls(item, out)),
        Value::Object(map) => [
            "url",
            "signedUrl",
            "signed_url",
            "downloadUrl",
            "download_url",
        ]
        .iter()
        .filter_map(|key| map.get(*key))
        .for_each(|item| push_urls(item, out)),
        _ => {}
    }
}

fn is_http(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// A safe file name from an address: the last part of its path, or a number.
fn file_name(url: &str, index: usize) -> String {
    // Some providers put the whole storage path in one encoded part.
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .replace("%2F", "/")
        .replace("%2f", "/");
    let last = path.rsplit('/').next().unwrap_or("");
    let clean: String = last
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    let clean = clean.trim_matches('.');
    if clean.is_empty() || !clean.contains('.') || path.matches('/').count() < 3 {
        format!("monid-file-{}", index + 1)
    } else {
        crate::util::truncate_chars(clean, 120)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_in_millionths_does_not_drift() {
        assert_eq!(dollars_to_micros(0.0035), 3_500);
        assert_eq!(dollars_to_micros(0.00015), 150);
        assert_eq!(dollars_to_micros(0.0000001), 1);
        assert_eq!(dollars_to_micros(-1.0), 0);
        assert_eq!(dollars_to_micros(f64::NAN), 0);
        assert_eq!(money_text(3_500), "$0.0035");
        assert_eq!(money_text(1_240_000), "$1.24");
        assert_eq!(money_text(150), "$0.00015");
        assert_eq!(money_text(0), "$0");
    }

    #[test]
    fn a_per_call_price_has_an_estimate() {
        let price = json!({"type": "PER_CALL", "amount": {"value": 0.015, "currency": "USD"}});
        let quote = quote(&price, &json!({}));
        assert_eq!(quote.rate, "$0.015 per call");
        assert_eq!(quote.estimate_micros, Some(15_000));
    }

    #[test]
    fn a_per_result_price_needs_a_limit_in_the_input() {
        let price = json!({"type": "PER_RESULT", "amount": {"value": 0.0016, "currency": "USD"}});
        assert_eq!(quote(&price, &json!({"body": {}})).estimate_micros, None);
        let one = quote(
            &price,
            &json!({"body": {"usernames": ["a"], "resultsLimit": 50}}),
        );
        assert_eq!(one.rate, "$0.0016 per result");
        assert_eq!(one.estimate_micros, Some(80_000));
        // The limit often applies per name, so three names can cost three times.
        let three = quote(
            &price,
            &json!({"body": {"usernames": ["a", "b", "c"], "resultsLimit": 50}}),
        );
        assert_eq!(three.estimate_micros, Some(240_000));
    }

    #[test]
    fn a_matrix_price_follows_the_model_in_the_input() {
        // The price of ElevenLabs text to speech, from a real inspect.
        let price = json!({
            "type": "PER_UNIT_MATRIX",
            "amount": {"value": 0.04, "currency": "USD"},
            "variants": [
                {"when": {"model_id": "eleven_v4"}, "price": {"type": "PER_UNIT", "amount": {"value": 0.08, "currency": "USD"}, "per": 1000, "unit": "character"}},
                {"when": {"model_id": "eleven_v4_turbo"}, "price": {"type": "PER_UNIT", "amount": {"value": 0.04, "currency": "USD"}, "per": 1000, "unit": "character"}}
            ]
        });
        let text = "x".repeat(620);
        let chosen = quote(
            &price,
            &json!({"body": {"model_id": "eleven_v4", "text": text}}),
        );
        assert_eq!(chosen.rate, "$0.08 per 1,000 characters");
        assert_eq!(chosen.estimate_micros, Some(49_600));
        let unknown_model = quote(&price, &json!({"body": {"text": "hello"}}));
        assert_eq!(unknown_model.rate, "From $0.04");
        assert_eq!(unknown_model.estimate_micros, None);
    }

    #[test]
    fn a_price_that_reads_the_output_has_no_estimate() {
        let price = json!({
            "type": "TIERED",
            "amount": {"value": 0.001, "currency": "USD"},
            "default": {"type": "PER_CALL", "amount": {"value": 0.001, "currency": "USD"}},
            "tiers": [{"label": "Crawled page", "selector": {"key": "crawledPages", "in": "output"}, "price": {"type": "PER_CALL", "amount": {"value": 0.001, "currency": "USD"}}}]
        });
        let quote = quote(&price, &json!({"body": {}}));
        assert_eq!(quote.rate, "$0.001 per call");
        assert_eq!(quote.estimate_micros, None);
    }

    #[test]
    fn an_unknown_price_says_so() {
        let quote = quote(&json!({"type": "SOMETHING_NEW"}), &json!({}));
        assert_eq!(quote.rate, "The price depends on the result");
        assert_eq!(quote.estimate_micros, None);
    }

    #[test]
    fn the_real_run_shape_parses() {
        // A real run from 2026-10-31, without the hints.
        let run: Run = serde_json::from_value(json!({
            "runId": "01M69S9MJYNYPQZWFZAXKS0BKM",
            "status": "COMPLETED",
            "providerResponse": {"httpStatus": 200},
            "price": {"type": "PER_RESULT", "amount": {"value": 0.0035, "currency": "USD"}},
            "cost": {"value": 0.0035, "currency": "USD"},
            "resultCount": 1,
            "output": {"data": {"image_urls": ["http://files.example/output/abc.jpeg?sig=1"]}}
        }))
        .unwrap();
        assert_eq!(run.status(), RunStatus::Completed);
        assert_eq!(run.cost_micros(), 3_500);
        assert!(!run.provider_failed());
        assert_eq!(
            output_files(&run.output),
            vec![OutputFile {
                url: "http://files.example/output/abc.jpeg?sig=1".to_owned(),
                name: "abc.jpeg".to_owned()
            }]
        );
    }

    #[test]
    fn a_run_status_that_is_new_counts_as_going() {
        assert_eq!(RunStatus::parse("RUNNING"), RunStatus::Going);
        assert_eq!(RunStatus::parse("READY"), RunStatus::Going);
        assert_eq!(RunStatus::parse("PAUSED_LATER"), RunStatus::Going);
        assert!(!RunStatus::Going.is_final());
        assert!(RunStatus::parse("BLOCKED").is_final());
        assert!(RunStatus::parse("TIME_OUT").is_final());
    }

    #[test]
    fn a_provider_error_is_seen() {
        let run: Run = serde_json::from_value(json!({
            "status": "COMPLETED",
            "providerResponse": {"httpStatus": 500},
        }))
        .unwrap();
        assert!(run.provider_failed());
        assert_eq!(run.cost_micros(), 0);
    }

    #[test]
    fn only_generated_files_are_fetched() {
        // A scraper's result has addresses that the hub must not fetch.
        let output = json!({
            "data": {
                "profilePicUrl": "https://cdn.example/face.jpg",
                "posts": [{"link": "http://10.0.0.1/admin"}]
            }
        });
        assert!(output_files(&output).is_empty());
        let audio =
            json!({"audio": {"format": "mp3", "url": "https://files.example/a/intro.mp3?x=1"}});
        assert_eq!(output_files(&audio)[0].name, "intro.mp3");
    }

    #[test]
    fn a_file_name_is_safe() {
        assert_eq!(file_name("https://x.example/a/..%2F..%2Fb.jpg", 0), "b.jpg");
        assert_eq!(
            file_name("https://x.example/a%2fb%2fc_d.jpeg?sig=1", 0),
            "c_d.jpeg"
        );
        assert_eq!(file_name("https://x.example/", 2), "monid-file-3");
        assert_eq!(
            file_name("https://x.example/noextension", 0),
            "monid-file-1"
        );
    }

    #[test]
    fn only_public_addresses_are_fetched() {
        for private in [
            "127.0.0.1",
            "10.0.0.1",
            "192.168.1.5",
            "172.16.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!is_public_ip(private.parse().unwrap()), "{private}");
        }
        for public in ["8.8.8.8", "2606:4700:4700::1111"] {
            assert!(is_public_ip(public.parse().unwrap()), "{public}");
        }
    }

    #[tokio::test]
    async fn a_download_refuses_a_private_host() {
        let err = download_file("http://127.0.0.1:9/secret.jpg", false)
            .await
            .unwrap_err();
        assert!(err.to_error_body().message.contains("public internet"));
    }

    #[test]
    fn a_run_id_cannot_leave_its_path() {
        assert_eq!(path_part("01M6/../wallet"), "01M6wallet");
    }

    #[test]
    fn errors_say_what_to_do() {
        let refused = status_error(401, &json!({}));
        assert!(is_unauthorized(&refused));
        let bad = status_error(400, &json!({"message": "prompt is required"}));
        assert_eq!(bad.code(), ErrorCode::Invalid);
        assert!(bad.to_error_body().message.contains("prompt is required"));
        assert!(bad.to_error_body().message.contains("nothing was charged"));
        let poor = status_error(402, &json!({}));
        assert!(poor.to_error_body().message.contains("app.monid.ai/wallet"));
    }

    /// A real call that costs money, for a person to run by hand. Set
    /// `CONSORT_MONID_REAL_KEY`, then run this test with `--ignored`.
    #[tokio::test]
    #[ignore = "it spends real money on Monid"]
    async fn a_real_run_gives_a_cost_and_a_file() {
        let key = std::env::var("CONSORT_MONID_REAL_KEY").unwrap();
        let client = Client::new(DEFAULT_BASE_URL, ApiKey::new(key)).unwrap();
        let before = client.balance().await.unwrap().balance.micros();
        let found = client.search("image generation", 5).await.unwrap();
        assert!(!found.is_empty());
        println!("name: {}", super::super::monid_run::tool_name(&found[0]));
        let provider = "minimax";
        let endpoint = "/v1/image_generation";
        let tool = client.inspect(provider, endpoint).await.unwrap();
        let input =
            json!({"body": {"model": "image-01", "prompt": "a small green teapot", "n": 1}});
        println!("rate: {:?}", quote(&tool.price, &input));
        let run = client.start_run(provider, endpoint, &input).await.unwrap();
        let run_id = run.run_id;
        let run = loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let run = client.run(&run_id).await.unwrap();
            if run.status().is_final() {
                break run;
            }
        };
        assert_eq!(run.status(), RunStatus::Completed);
        assert!(!run.provider_failed());
        let files = output_files(&run.output);
        assert_eq!(files.len(), 1, "{:?}", run.output);
        let file = download_file(&files[0].url, false).await.unwrap();
        assert!(!file.bytes.is_empty());
        println!(
            "file: {} {} bytes {}",
            files[0].name,
            file.bytes.len(),
            file.mime
        );
        // The wallet settles a short time after the run ends.
        let mut fell = 0;
        for _ in 0..30 {
            fell = before - client.balance().await.unwrap().balance.micros();
            if fell != 0 {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        println!("cost: {} balance fell by {}", run.cost_micros(), fell);
        assert_eq!(fell, run.cost_micros());
    }
}
