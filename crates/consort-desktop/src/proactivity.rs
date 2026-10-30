//! The assistant on its own, in the desktop app: the state and the actions
//! of Settings > Proactivity, the level select, the quick replies, the
//! ratings of proactive messages, and the designed screens.

use consort_client::Command;
use consort_client::proactivity::{ProactiveKind, ProactiveRating, ProactivityLevel};
use consort_client::proactivity::{ProactivityChanges, SnoozeOption};
use gpui_kit::{Context, Window};

use crate::app::{ConsortApp, SettingsSection};

/// The `--screen` names of the proactivity designs.
pub const SCREENS: &[&str] = &[
    "proactivity",
    "proactive-messages",
    "routines-follow-ups",
    "channel-greeting",
    "channel-writing",
    "channel-question",
];

/// The select of the quiet hours start.
pub const QUIET_START_SELECT: &str = "proactivity-quiet-start";
/// The select of the quiet hours end.
pub const QUIET_END_SELECT: &str = "proactivity-quiet-end";
/// The time zone select.
pub const ZONE_SELECT: &str = "proactivity-zone";
/// The channel filter of the log.
pub const LOG_CHANNEL_SELECT: &str = "proactivity-log-channel";
/// The kind filter of the log.
pub const LOG_KIND_SELECT: &str = "proactivity-log-kind";

/// The level selects of the rows of "Each channel". A select needs a
/// static ID for its menu anchor, so each row takes the slot of its index;
/// the option names the channel, so a pick never moves to another row.
pub const LEVEL_SELECTS: [&str; 32] = [
    "level-0", "level-1", "level-2", "level-3", "level-4", "level-5", "level-6", "level-7",
    "level-8", "level-9", "level-10", "level-11", "level-12", "level-13", "level-14", "level-15",
    "level-16", "level-17", "level-18", "level-19", "level-20", "level-21", "level-22", "level-23",
    "level-24", "level-25", "level-26", "level-27", "level-28", "level-29", "level-30", "level-31",
];

/// The level select of the channel settings dialog. A pick goes into the
/// form, and Save changes sends it.
pub const CHANNEL_SETTINGS_LEVEL_SELECT: &str = "channel-settings-proactivity";

/// The value of "Default for this channel" in a level select.
pub const DEFAULT_LEVEL: &str = "default";

/// The common time zones of the time zone select. This computer's zone and
/// the zone that is set come first.
const ZONES: [&str; 12] = [
    "UTC",
    "America/New_York",
    "America/Chicago",
    "America/Denver",
    "America/Los_Angeles",
    "America/Sao_Paulo",
    "Europe/London",
    "Europe/Berlin",
    "Asia/Kolkata",
    "Asia/Singapore",
    "Asia/Tokyo",
    "Australia/Sydney",
];

/// What the desktop keeps for the proactivity screens.
#[derive(Clone, Debug, Default)]
pub struct ProactivityUi {
    /// The channel filter of the log. `None` is all channels.
    pub log_channel: Option<String>,
    /// The kind filter of the log. `None` is all kinds.
    pub log_kind: Option<ProactiveKind>,
    /// The proactive message under the pointer: its Useful and Too noisy
    /// show.
    pub hovered: Option<String>,
    /// The follow-up row under the pointer: its Cancel shows.
    pub hovered_follow_up: Option<String>,
    /// True once the app asked for the log.
    pub log_asked: bool,
}

/// The option of a level select: `<channel-id>:<level>`, or
/// `<channel-id>:default`.
pub fn level_option(channel_id: &str, level: Option<ProactivityLevel>) -> String {
    format!(
        "{channel_id}:{}",
        level.map_or(DEFAULT_LEVEL, ProactivityLevel::as_str)
    )
}

/// Reads the option of a level select. `Some((channel, None))` is the
/// default for the channel.
pub fn parse_level_option(option: &str) -> Option<(String, Option<ProactivityLevel>)> {
    let (channel, level) = option.rsplit_once(':')?;
    let level = match level {
        DEFAULT_LEVEL => None,
        other => Some(ProactivityLevel::parse(other)?),
    };
    Some((channel.to_owned(), level))
}

/// The times of the quiet hours selects: every 30 minutes, and the time
/// that is set when it is between them.
pub fn quiet_times(current: &str) -> Vec<String> {
    let mut times: Vec<String> = (0..48)
        .map(|i| format!("{:02}:{:02}", i / 2, (i % 2) * 30))
        .collect();
    if !times.iter().any(|t| t == current) && !current.is_empty() {
        times.push(current.to_owned());
        times.sort();
    }
    times
}

/// The zones of the time zone select: this computer's zone, the zone that
/// is set, then the common zones, each once.
pub fn zone_options(current: &str) -> Vec<String> {
    let mut zones: Vec<String> = Vec::new();
    for zone in [consort_client::admin::local_time_zone(), current.to_owned()]
        .into_iter()
        .chain(ZONES.iter().map(|z| (*z).to_owned()))
    {
        if !zone.is_empty() && !zones.contains(&zone) {
            zones.push(zone);
        }
    }
    zones
}

/// The kinds of the kind filter.
pub const LOG_KINDS: [ProactiveKind; 7] = ProactiveKind::ALL;

impl ConsortApp {
    /// Runs one proactivity command. A failure shows as a notice.
    fn run_proactivity(&mut self, command: Command, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.spawn_command(cx, async move { client.run(command).await });
    }

    /// Opens Settings > Proactivity and loads the log once.
    pub fn open_proactivity(&mut self, cx: &mut Context<Self>) {
        self.open_settings(SettingsSection::Proactivity, cx);
        self.load_proactivity_log(cx);
    }

    /// Loads the newest page of the log, once per app run.
    pub fn load_proactivity_log(&mut self, cx: &mut Context<Self>) {
        if self.proactivity.log_asked {
            return;
        }
        self.proactivity.log_asked = true;
        self.run_proactivity(Command::LoadProactivityLog, cx);
    }

    pub fn set_proactivity_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        self.run_proactivity(
            Command::SetProactivity(ProactivityChanges {
                paused: Some(paused),
                ..ProactivityChanges::default()
            }),
            cx,
        );
    }

    pub fn snooze_proactivity(&mut self, option: SnoozeOption, cx: &mut Context<Self>) {
        self.run_proactivity(Command::SnoozeProactivity(option), cx);
    }

    pub fn cancel_all_follow_ups(&mut self, channel_id: Option<String>, cx: &mut Context<Self>) {
        self.run_proactivity(Command::CancelAllFollowUps { channel_id }, cx);
    }

    /// Cancels one follow-up: it is a routine.
    pub fn cancel_follow_up(&mut self, routine_id: String, cx: &mut Context<Self>) {
        self.run_proactivity(Command::DeleteRoutine { routine_id }, cx);
    }

    pub fn set_channel_level(
        &mut self,
        channel_id: String,
        level: Option<ProactivityLevel>,
        cx: &mut Context<Self>,
    ) {
        self.run_proactivity(Command::SetChannelProactivity { channel_id, level }, cx);
    }

    /// Useful or Too noisy. A second click on the same rating takes it back.
    pub fn rate_message(
        &mut self,
        message_id: String,
        rating: ProactiveRating,
        current: Option<ProactiveRating>,
        cx: &mut Context<Self>,
    ) {
        let rating = (current != Some(rating)).then_some(rating);
        self.run_proactivity(Command::RateMessage { message_id, rating }, cx);
    }

    /// A quick reply: a send chip sends its label as your message. "I will
    /// type it" only moves the focus to the composer.
    pub fn use_suggestion(
        &mut self,
        label: String,
        type_it: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if type_it {
            self.inputs
                .composer
                .update(cx, |input, cx| input.focus(window, cx));
            return;
        }
        let client = self.client.clone();
        let channel_id = self.channel_id.clone();
        self.spawn_command(
            cx,
            async move { client.send_message(channel_id, label).await },
        );
    }

    /// A pick from one of the proactivity selects. Returns false for
    /// another select.
    pub fn pick_proactivity_select(
        &mut self,
        select: &str,
        option: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        match select {
            QUIET_START_SELECT => self.run_proactivity(
                Command::SetProactivity(ProactivityChanges {
                    quiet_start: Some(option.to_owned()),
                    ..ProactivityChanges::default()
                }),
                cx,
            ),
            QUIET_END_SELECT => self.run_proactivity(
                Command::SetProactivity(ProactivityChanges {
                    quiet_end: Some(option.to_owned()),
                    ..ProactivityChanges::default()
                }),
                cx,
            ),
            ZONE_SELECT => self.run_proactivity(
                Command::SetProactivity(ProactivityChanges {
                    time_zone: Some(option.to_owned()),
                    ..ProactivityChanges::default()
                }),
                cx,
            ),
            LOG_CHANNEL_SELECT => {
                self.proactivity.log_channel = (!option.is_empty()).then(|| option.to_owned());
            }
            LOG_KIND_SELECT => self.proactivity.log_kind = ProactiveKind::parse(option),
            select if LEVEL_SELECTS.contains(&select) => match parse_level_option(option) {
                Some((channel, level)) => self.set_channel_level(channel, level, cx),
                None => return false,
            },
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Opens a proactivity design screen. Returns false for another screen.
    pub fn open_proactive_screen(
        &mut self,
        screen: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use consort_client::mock_proactivity::{
            CHECK_IN, CHECK_IN_MESSAGE, LIGHTS, READING, TUTTI,
        };
        match screen {
            "proactivity" => self.open_proactivity(cx),
            "proactive-messages" => {
                self.open_channel(TUTTI.into(), window, cx);
                // The hover state of the design: Useful on the check-in.
                self.proactivity.hovered = Some(CHECK_IN_MESSAGE.into());
            }
            "routines-follow-ups" => {
                self.open_channel(LIGHTS.into(), window, cx);
                self.open_channel_tab(crate::app::ChannelTab::Routines, cx);
                self.knowledge.hover = Some(CHECK_IN.into());
                self.proactivity.hovered_follow_up =
                    Some(consort_client::mock_proactivity::FOLLOW_UP_QUOTE.into());
            }
            "channel-greeting" | "channel-writing" | "channel-question" => {
                self.open_channel(READING.into(), window, cx);
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_option_names_its_channel_and_reads_back() {
        let option = level_option("chn_1", Some(ProactivityLevel::Eager));
        assert_eq!(option, "chn_1:eager");
        assert_eq!(
            parse_level_option(&option),
            Some(("chn_1".to_owned(), Some(ProactivityLevel::Eager)))
        );
        assert_eq!(
            parse_level_option(&level_option("chn_1", None)),
            Some(("chn_1".to_owned(), None))
        );
        assert_eq!(parse_level_option("chn_1:loud"), None);
    }

    #[test]
    fn the_time_and_zone_selects_offer_the_value_that_is_set() {
        let times = quiet_times("21:15");
        assert!(times.contains(&"21:15".to_owned()));
        assert_eq!(times.len(), 49);
        assert_eq!(times[0], "00:00");
        let zones = zone_options("Pacific/Auckland");
        assert!(zones.contains(&"Pacific/Auckland".to_owned()));
        assert_eq!(
            zones.iter().filter(|z| z.as_str() == "UTC").count(),
            1,
            "each zone once"
        );
    }
}
