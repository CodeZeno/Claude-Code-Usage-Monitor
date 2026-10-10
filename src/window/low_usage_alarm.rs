//! The low-usage alarm: a sound when a quota becomes almost spent, and
//! the `alarm.*` theme bindings that keep edge-docked surfaces revealed.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use crate::theme_engine::AlarmScan;

/// How long "Snooze alarm (1 hour)" lets surfaces hide again.
pub(super) const SNOOZE: Duration = Duration::from_secs(60 * 60);

/// How far past its reset time a stored crossing survives, so a slightly fast
/// local clock does not drop it the instant the reset passes.
const RESET_GRACE: Duration = Duration::from_secs(5 * 60);

/// The alarm sounds at most this often, so providers finishing a few seconds
/// apart do not each play it.
const SOUND_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Default)]
pub(super) struct LowUsageAlarm {
    /// Quotas that crossed in and have not recovered, with their source and
    /// the reset time last seen. Independent of the theme and of which
    /// account is selected.
    crossed: BTreeMap<String, (String, Option<SystemTime>)>,
    snoozed_until: Option<Instant>,
    /// Monotonic, so moving the clock back cannot sound the alarm early.
    last_sound: Option<Instant>,
}

impl LowUsageAlarm {
    /// Take the latest readings (`now` is wall-clock time for quota reset
    /// times, `mono` measures the sound spacing). Returns whether the sound
    /// should play: only when a quota newly crosses in, and at most once a
    /// minute even when several limits cross. A crossing ends any snooze. A
    /// fresh low reading always counts. A stale one (kept by the poller from
    /// a failed refresh) never creates a crossing and only keeps one until
    /// its own or the stored reset time has passed (with a grace for clock
    /// skew). A stored crossing also clears when a good reading shows the
    /// quota above the zone. A missing reading, a failed poll or a theme or
    /// account switch is no evidence; a source gone from the data (account
    /// removed, provider disabled) is dropped. A reading without a reset time
    /// keeps the last one seen.
    pub fn update(&mut self, scan: &AlarmScan, now: SystemTime, mono: Instant) -> bool {
        let mut crossed = false;
        let mut confirmed = std::collections::BTreeSet::new();
        let expired = |at: Option<SystemTime>| at.is_some_and(|at| now >= at + RESET_GRACE);
        for reading in &scan.readings {
            if reading.low && !reading.stale {
                let previous = self.crossed.get(&reading.key).and_then(|(_, at)| *at);
                let record = (reading.owner.clone(), reading.resets_at.or(previous));
                crossed |= self.crossed.insert(reading.key.clone(), record).is_none();
                confirmed.insert(reading.key.as_str());
            } else if !(reading.low
                && self.crossed.contains_key(&reading.key)
                && !expired(reading.resets_at))
            {
                // A stale low reading keeps an existing record (below) only
                // while its own reset has not passed.
                self.crossed.remove(&reading.key);
            }
        }
        self.crossed.retain(|key, (owner, at)| {
            scan.owners.contains(owner) && (confirmed.contains(key.as_str()) || !expired(*at))
        });
        if crossed || self.crossed.is_empty() {
            self.snoozed_until = None;
        }
        // Crossings in quick succession (providers publish one by one) still
        // update the state above, but share one sound.
        let sound = crossed
            && self
                .last_sound
                .is_none_or(|at| mono.saturating_duration_since(at) >= SOUND_INTERVAL);
        if sound {
            self.last_sound = Some(mono);
        }
        sound
    }

    /// Forget crossings and snooze, as when the alarm is turned off. The
    /// sound spacing stays, so switching off and on cannot dodge it.
    pub fn clear(&mut self) {
        *self = Self {
            last_sound: self.last_sound,
            ..Self::default()
        };
    }

    /// Let surfaces hide for an hour while the current limits stay low.
    pub fn snooze(&mut self, now: Instant) {
        if !self.crossed.is_empty() {
            self.snoozed_until = now.checked_add(SNOOZE);
        }
    }

    pub fn snoozed(&self, now: Instant) -> bool {
        !self.crossed.is_empty() && self.snoozed_until.is_some_and(|until| now < until)
    }

    /// A limit is almost spent and the alarm is not snoozed.
    pub fn active(&self, now: Instant) -> bool {
        !self.crossed.is_empty() && !self.snoozed(now)
    }

    /// When an active snooze runs out, so the surface can be revealed again.
    pub fn snooze_remaining(&self, now: Instant) -> Option<Duration> {
        self.snoozed_until
            .filter(|_| self.snoozed(now))
            .map(|until| until - now)
    }
}

/// The alarm sound, synthesized for this project (see `src/assets/README.md`).
/// Static, so it outlives the asynchronous playback.
static ALARM_WAV: &[u8] = include_bytes!("../assets/alarm-warning-pulse.wav");

/// What the alarm asks the system to play.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Source {
    /// The bundled sound, played from memory.
    Embedded(&'static [u8]),
    /// The sound scheme's "Alarm 1", else its "Critical Stop".
    Scheme,
}

/// Play the alarm once, without blocking: the bundled sound, and only if that
/// fails the sound scheme's alarm. A scheme with neither sound, such as "No
/// Sounds", keeps the alarm silent, as it always has.
pub(super) fn play_sound() {
    let played = emit(Source::Embedded(ALARM_WAV)) || emit(Source::Scheme);
    #[cfg(not(test))]
    crate::diagnose::log(format!("low-usage alarm sounded: {played}"));
    #[cfg(test)]
    let _ = played;
}

// The windows crate binds PlaySound only under its Media feature.
#[cfg(not(test))]
#[link(name = "winmm")]
extern "system" {
    fn PlaySoundW(sound: windows::core::PCWSTR, module: *mut std::ffi::c_void, flags: u32) -> i32;
}

/// Whether the active sound scheme has a sound for this event.
#[cfg(not(test))]
fn scheme_has_sound(event: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::*;
    let path = crate::native_interop::wide_str(&format!(
        r"AppEvents\Schemes\Apps\.Default\{event}\.Current"
    ));
    unsafe {
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(path.as_ptr()),
            None,
            KEY_READ,
            &mut hkey,
        )
        .is_err()
        {
            return false;
        }
        // The default value is the .wav path; empty (just a terminator) = "(None)".
        let mut size = 0u32;
        let result = RegQueryValueExW(hkey, PCWSTR::null(), None, None, None, Some(&mut size));
        let _ = RegCloseKey(hkey);
        result.is_ok() && size > std::mem::size_of::<u16>() as u32
    }
}

/// The far boundary: ask the system to play one source. True when the alarm
/// is handled, including a scheme the user silenced on purpose.
#[cfg(not(test))]
fn emit(source: Source) -> bool {
    const SND_ASYNC: u32 = 0x0001;
    const SND_NODEFAULT: u32 = 0x0002;
    const SND_MEMORY: u32 = 0x0004;
    const SND_ALIAS: u32 = 0x0001_0000;
    const SND_SYSTEM: u32 = 0x0020_0000;
    match source {
        Source::Embedded(bytes) => {
            // "No Sounds": the user silenced Windows sounds, so stay quiet.
            if !scheme_has_sound("Notification.Looping.Alarm") && !scheme_has_sound("SystemHand") {
                return true;
            }
            unsafe {
                PlaySoundW(
                    windows::core::PCWSTR(bytes.as_ptr().cast()),
                    std::ptr::null_mut(),
                    SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
                ) != 0
            }
        }
        Source::Scheme => ["Notification.Looping.Alarm", "SystemHand"]
            .into_iter()
            .any(|alias| {
                let alias = crate::native_interop::wide_str(alias);
                unsafe {
                    PlaySoundW(
                        windows::core::PCWSTR(alias.as_ptr()),
                        std::ptr::null_mut(),
                        SND_ALIAS | SND_ASYNC | SND_NODEFAULT | SND_SYSTEM,
                    ) != 0
                }
            }),
    }
}

#[cfg(test)]
thread_local! {
    /// Tests count alarms instead of making noise.
    pub(super) static SOUNDED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// What the last alarm asked the system to play.
    static LAST_SOURCE: std::cell::Cell<Option<Source>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn emit(source: Source) -> bool {
    SOUNDED.with(|count| count.set(count.get() + 1));
    LAST_SOURCE.with(|last| last.set(Some(source)));
    true
}

/// Tests drive the wall clock; this maps it onto a monotonic clock that moves
/// the same amount.
#[cfg(test)]
pub(super) fn mono_for(now: SystemTime) -> Instant {
    static BASE: std::sync::OnceLock<(SystemTime, Instant)> = std::sync::OnceLock::new();
    let (system, mono) = *BASE.get_or_init(|| (SystemTime::now(), Instant::now()));
    mono + now.duration_since(system).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(alarm: &mut LowUsageAlarm, scan: &AlarmScan, now: SystemTime) -> bool {
        alarm.update(scan, now, mono_for(now))
    }

    use crate::models::{AccountUsage, AppUsageData, UsageData, UsageLimit, UsageSection};
    use crate::providers::{ProviderId, ProviderSet};

    fn section(used: f64, resets_at: Option<SystemTime>) -> UsageSection {
        UsageSection {
            available: true,
            percentage: used,
            resets_at,
        }
    }

    /// One Claude account with a weekly reading and an optional Fable cap.
    fn account(
        id: &str,
        weekly: f64,
        fable: Option<f64>,
        resets_at: Option<SystemTime>,
    ) -> AccountUsage {
        AccountUsage {
            provider: ProviderId::Claude,
            profile: crate::accounts::AccountProfile {
                id: id.into(),
                name: id.into(),
                enabled: true,
                ..Default::default()
            },
            source_signature: String::new(),
            source_path: None,
            usage: Some(UsageData {
                weekly: section(weekly, resets_at),
                limits: fable
                    .map(|used| UsageLimit {
                        key: "weekly_scoped_fable".into(),
                        kind: "weekly_scoped".into(),
                        model: Some("Fable".into()),
                        usage: section(used, resets_at),
                        ..Default::default()
                    })
                    .into_iter()
                    .collect(),
                ..Default::default()
            }),
            error: None,
            selected: false,
        }
    }

    fn scan(accounts: Vec<AccountUsage>) -> AlarmScan {
        let mut data = AppUsageData::default();
        data.accounts = accounts;
        crate::theme_engine::alarm_scan(&data, ProviderSet::from_enabled([ProviderId::Claude]))
    }

    /// The poller kept these readings from an earlier poll.
    fn stale_scan(mut accounts: Vec<AccountUsage>) -> AlarmScan {
        for account in &mut accounts {
            account.usage.as_mut().unwrap().stale = true;
        }
        scan(accounts)
    }

    fn fable(used: f64) -> AlarmScan {
        scan(vec![account("a", 10.0, Some(used), None)])
    }

    #[test]
    fn sounds_once_per_crossing_not_on_every_poll() {
        let now = SystemTime::now();
        let mut alarm = LowUsageAlarm::default();
        assert!(!up(&mut alarm, &fable(50.0), now));
        assert!(up(&mut alarm, &fable(96.0), now), "crossing in sounds");
        for _ in 0..5 {
            assert!(
                !up(&mut alarm, &fable(96.0), now),
                "repeat polls stay quiet"
            );
        }
        // A second quota crossing sounds; leaving is silent.
        let both = scan(vec![account("a", 97.0, Some(96.0), None)]);
        let later = now + Duration::from_secs(120);
        assert!(up(&mut alarm, &both, later));
        assert!(!up(&mut alarm, &fable(96.0), later));
    }

    #[test]
    fn a_steady_low_quota_stays_silent_across_theme_and_account_switches() {
        let now = SystemTime::now();
        let low = |ids: &[&str]| {
            scan(
                ids.iter()
                    .map(|id| account(id, 10.0, Some(96.0), None))
                    .collect(),
            )
        };
        let mut alarm = LowUsageAlarm::default();
        assert!(up(&mut alarm, &fable(96.0), now));
        alarm.snooze(Instant::now());
        // The theme never reaches the alarm: renders with any theme (Top Bar,
        // Classic, Top Bar) feed it the same scan.
        for _ in 0..3 {
            assert!(!up(&mut alarm, &fable(96.0), now));
        }
        assert!(
            alarm.snoozed(Instant::now()),
            "theme switches keep the snooze"
        );
        // Accounts A -> B -> A, both low: B's own crossing sounds once, and
        // switching back is silent.
        assert!(up(
            &mut alarm,
            &low(&["a", "b"]),
            now + Duration::from_secs(120)
        ));
        alarm.snooze(Instant::now());
        for ids in [["a", "b"], ["b", "a"], ["a", "b"]] {
            assert!(!up(&mut alarm, &low(&ids), now));
        }
        assert!(alarm.snoozed(Instant::now()));
    }

    #[test]
    fn a_poll_gap_or_failed_poll_is_not_a_recovery() {
        let now = SystemTime::now();
        let mut alarm = LowUsageAlarm::default();
        assert!(up(&mut alarm, &fable(96.0), now));
        alarm.snooze(Instant::now());
        // The account reports nothing (failed poll): still the same crossing.
        let mut failed = account("a", 10.0, Some(96.0), None);
        failed.usage = None;
        failed.error = Some(crate::poller::PollError::NetworkError);
        assert!(!up(&mut alarm, &scan(vec![failed]), now));
        assert!(alarm.snoozed(Instant::now()) && !alarm.crossed.is_empty());
        // A reading without that cap at all (quota missing) is no recovery either.
        assert!(!up(
            &mut alarm,
            &scan(vec![account("a", 10.0, None, None)]),
            now
        ));
        assert!(
            !up(&mut alarm, &fable(96.0), now),
            "back at 96% stays silent"
        );
        assert!(alarm.snoozed(Instant::now()));
    }

    #[test]
    fn recovery_or_a_passed_reset_lets_the_quota_sound_again() {
        let now = SystemTime::now();
        let mut alarm = LowUsageAlarm::default();
        assert!(up(&mut alarm, &fable(96.0), now));
        alarm.snooze(Instant::now());
        // A good reading at 10% used is recovery; low again sounds, snooze gone.
        assert!(!up(&mut alarm, &fable(10.0), now));
        assert!(!alarm.snoozed(Instant::now()));
        assert!(up(&mut alarm, &fable(96.0), now + Duration::from_secs(120)));

        // The reset time passing is recovery even with no new reading.
        let reset = now + Duration::from_secs(60);
        let before = scan(vec![account("a", 10.0, Some(96.0), Some(reset))]);
        let mut alarm = LowUsageAlarm::default();
        assert!(up(&mut alarm, &before, now));
        alarm.snooze(Instant::now());
        let later = reset + Duration::from_secs(6 * 60);
        let next = now + Duration::from_secs(3600);
        let gone = AlarmScan {
            owners: before.owners.clone(),
            ..Default::default()
        };
        assert!(!up(&mut alarm, &gone, later), "nothing left to watch");
        assert!(!alarm.active(Instant::now()));
        let again = scan(vec![account("a", 10.0, Some(96.0), Some(next))]);
        assert!(
            up(&mut alarm, &again, later),
            "low again after the reset sounds"
        );
    }

    #[test]
    fn a_fresh_low_reading_counts_with_a_reset_just_behind_the_clock() {
        let now = SystemTime::now();
        let behind = now - Duration::from_secs(2 * 60);
        let skewed = || scan(vec![account("a", 10.0, Some(96.0), Some(behind))]);
        let mut alarm = LowUsageAlarm::default();
        assert!(up(&mut alarm, &skewed(), now), "eligible on its own");
        assert!(alarm.active(Instant::now()));
        // Still confirmed low each poll: held, and silent after the first.
        assert!(!up(&mut alarm, &skewed(), now));
        assert!(alarm.active(Instant::now()));
    }

    #[test]
    fn a_carried_forward_reading_past_its_reset_expires_and_a_new_reset_crosses_again() {
        let now = SystemTime::now();
        let reset = now + Duration::from_secs(60);
        let cached = stale_scan(vec![account("a", 10.0, Some(96.0), Some(reset))]);
        let mut alarm = LowUsageAlarm::default();
        assert!(
            up(
                &mut alarm,
                &scan(vec![account("a", 10.0, Some(96.0), Some(reset))]),
                now
            ),
            "fresh"
        );
        // A failed poll leaves the same cached reading in the scan.
        let stale = reset + Duration::from_secs(6 * 60);
        assert!(!up(&mut alarm, &cached, stale));
        assert!(!alarm.active(Instant::now()), "stale reading keeps nothing");
        assert!(alarm.crossed.is_empty());
        let fresh = scan(vec![account(
            "a",
            10.0,
            Some(96.0),
            Some(stale + Duration::from_secs(3600)),
        )]);
        assert!(up(&mut alarm, &fresh, stale), "a new reset crosses again");
        assert!(alarm.active(Instant::now()));
    }

    #[test]
    fn a_stale_low_reading_never_creates_and_expires_without_a_reset_time() {
        let now = SystemTime::now();
        let reset = now + Duration::from_secs(60);
        let mut alarm = LowUsageAlarm::default();
        let stale_new = stale_scan(vec![account("a", 10.0, Some(96.0), None)]);
        assert!(!up(&mut alarm, &stale_new, now), "stale never crosses in");
        assert!(alarm.crossed.is_empty());
        let fresh = scan(vec![account("a", 10.0, Some(96.0), Some(reset))]);
        assert!(up(&mut alarm, &fresh, now));
        let resetless = stale_scan(vec![account("a", 10.0, Some(96.0), None)]);
        let inside = reset + Duration::from_secs(4 * 60);
        assert!(!up(&mut alarm, &resetless, inside));
        assert!(alarm.active(Instant::now()), "kept inside the grace");
        let past = reset + Duration::from_secs(6 * 60);
        assert!(!up(&mut alarm, &resetless, past));
        assert!(alarm.crossed.is_empty(), "retained reset + grace passed");
    }

    #[test]
    fn a_fresh_low_reading_counts_whatever_its_reset_says() {
        let now = SystemTime::now();
        let behind = now - Duration::from_secs(6 * 60);
        let mut alarm = LowUsageAlarm::default();
        let fresh = scan(vec![account("a", 10.0, Some(97.0), Some(behind))]);
        assert!(up(&mut alarm, &fresh, now), "counts and sounds");
        assert!(alarm.active(Instant::now()));
    }

    #[test]
    fn turning_the_alarm_off_and_on_keeps_the_sound_spacing() {
        let now = SystemTime::now();
        let mut alarm = LowUsageAlarm::default();
        assert!(up(&mut alarm, &fable(96.0), now));
        alarm.clear();
        assert!(alarm.crossed.is_empty());
        let soon = now + Duration::from_secs(10);
        assert!(!up(&mut alarm, &fable(96.0), soon), "cooldown survives");
        assert!(alarm.active(Instant::now()), "still held open");
        alarm.clear();
        let later = now + Duration::from_secs(70);
        assert!(up(&mut alarm, &fable(96.0), later));
    }

    #[test]
    fn moving_the_clock_back_does_not_sound_again() {
        let now = SystemTime::now();
        let t = Instant::now();
        let mut alarm = LowUsageAlarm::default();
        assert!(alarm.update(&fable(96.0), now, t));
        alarm.clear();
        let back = now - Duration::from_secs(3600);
        assert!(!alarm.update(&fable(96.0), back, t + Duration::from_secs(5)));
        alarm.clear();
        assert!(alarm.update(&fable(96.0), back, t + Duration::from_secs(61)));
    }

    #[test]
    fn crossings_within_a_minute_share_one_sound_and_a_later_one_sounds() {
        let now = SystemTime::now();
        let mut alarm = LowUsageAlarm::default();
        assert!(up(&mut alarm, &fable(96.0), now));
        alarm.snooze(Instant::now());
        let second = scan(vec![account("a", 97.0, Some(96.0), None)]);
        assert!(!up(&mut alarm, &second, now + Duration::from_secs(3)));
        assert!(alarm.active(Instant::now()), "still ends the snooze");
        let third = scan(vec![
            account("a", 97.0, Some(96.0), None),
            account("b", 98.0, None, None),
        ]);
        assert!(up(&mut alarm, &third, now + Duration::from_secs(120)));
    }

    #[test]
    fn a_stored_crossing_survives_a_reset_inside_the_grace_and_expires_after() {
        let now = SystemTime::now();
        let reset = now + Duration::from_secs(60);
        let low = scan(vec![account("a", 10.0, Some(96.0), Some(reset))]);
        let none = AlarmScan {
            owners: low.owners.clone(),
            ..Default::default()
        };
        let mut alarm = LowUsageAlarm::default();
        assert!(up(&mut alarm, &low, now));
        up(&mut alarm, &none, reset + Duration::from_secs(4 * 60));
        assert!(alarm.active(Instant::now()), "inside the grace");
        up(&mut alarm, &none, reset + Duration::from_secs(6 * 60));
        assert!(!alarm.active(Instant::now()), "past the grace");
    }

    #[test]
    fn a_reading_without_a_reset_time_keeps_the_last_known_one() {
        let now = SystemTime::now();
        let reset = now + Duration::from_secs(60);
        let with = scan(vec![account("a", 10.0, Some(96.0), Some(reset))]);
        let without = scan(vec![account("a", 10.0, Some(96.0), None)]);
        let none = AlarmScan {
            owners: with.owners.clone(),
            ..Default::default()
        };
        let mut alarm = LowUsageAlarm::default();
        assert!(up(&mut alarm, &with, now));
        assert!(!up(&mut alarm, &without, now));
        up(&mut alarm, &none, reset + Duration::from_secs(6 * 60));
        assert!(
            !alarm.active(Instant::now()),
            "the kept reset time still expires the record"
        );
    }

    #[test]
    fn several_quotas_crossing_in_one_poll_report_one_sound() {
        let now = SystemTime::now();
        let mut alarm = LowUsageAlarm::default();
        let many = scan(vec![
            account("a", 97.0, Some(96.0), None),
            account("b", 98.0, Some(99.0), None),
        ]);
        assert!(up(&mut alarm, &many, now), "one sound for the whole poll");
        assert_eq!(alarm.crossed.len(), 4);
        assert!(!up(&mut alarm, &many, now));
    }

    #[test]
    fn a_non_selected_accounts_low_weekly_alarms_and_removed_sources_are_dropped() {
        let now = SystemTime::now();
        let mut alarm = LowUsageAlarm::default();
        let work = account("work", 98.0, None, None);
        assert!(up(
            &mut alarm,
            &scan(vec![account("a", 10.0, None, None), work]),
            now
        ));
        assert!(alarm.active(Instant::now()));
        // The account is removed from the settings: nothing holds the bar.
        assert!(!up(
            &mut alarm,
            &scan(vec![account("a", 10.0, None, None)]),
            now
        ));
        assert!(!alarm.active(Instant::now()));
    }

    #[test]
    fn active_while_a_quota_is_low_and_snooze_lasts_an_hour() {
        let now = SystemTime::now();
        let t = Instant::now();
        let mut alarm = LowUsageAlarm::default();
        assert!(!alarm.active(t));
        alarm.snooze(t);
        assert!(!alarm.snoozed(t), "nothing to snooze");
        up(&mut alarm, &fable(96.0), now);
        assert!(alarm.active(t));
        alarm.snooze(t);
        assert!(alarm.snoozed(t) && !alarm.active(t));
        assert_eq!(alarm.snooze_remaining(t), Some(SNOOZE));
        assert!(!alarm.active(t + SNOOZE - Duration::from_secs(1)));
        assert!(
            alarm.active(t + SNOOZE),
            "an hour later it holds the bar again"
        );
        assert_eq!(alarm.snooze_remaining(t + SNOOZE), None);
        // A different quota crossing during the snooze alarms at once.
        alarm.snooze(t);
        let later = now + Duration::from_secs(120);
        assert!(up(
            &mut alarm,
            &scan(vec![account("a", 97.0, Some(96.0), None)]),
            later
        ));
        assert!(alarm.active(t) && !alarm.snoozed(t));
        // Once every quota has recovered there is nothing left snoozed.
        alarm.snooze(t);
        up(&mut alarm, &fable(10.0), now);
        assert!(!alarm.snoozed(t) && !alarm.active(t));
    }

    #[test]
    fn bundled_sound_is_a_pcm_wav_of_the_expected_size() {
        let wav = ALARM_WAV;
        assert_eq!(wav.len(), 84_716);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize,
            wav.len() - 8
        );
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        let u16_at = |at: usize| u16::from_le_bytes(wav[at..at + 2].try_into().unwrap());
        assert_eq!(u16_at(20), 1, "PCM");
        assert_eq!(u16_at(22), 1, "mono");
        assert_eq!(
            u32::from_le_bytes(wav[24..28].try_into().unwrap()),
            44_100,
            "sample rate"
        );
        assert_eq!(u16_at(34), 16, "bits per sample");
    }

    #[test]
    fn the_alarm_plays_the_embedded_sound_once() {
        SOUNDED.with(|count| count.set(0));
        LAST_SOURCE.with(|last| last.set(None));
        play_sound();
        assert_eq!(SOUNDED.with(std::cell::Cell::get), 1);
        match LAST_SOURCE.with(std::cell::Cell::get) {
            Some(Source::Embedded(bytes)) => assert!(std::ptr::eq(bytes, ALARM_WAV)),
            other => panic!("expected the embedded sound, got {other:?}"),
        }
    }
}
