//! Cards for the quota view and the `quota` command, and the JSON report. Percentages are what
//! is left, like quota-axi's view: green from 50, yellow from 20, red below.

use super::pace::{self, Headline, Runway};
use super::{Provider, Snapshot, Status, Time, Window, live};
use crate::ui::{OVERLAY, SUBTEXT, SURFACE2, TEXT, truncate};
use chrono::Local;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde::Serialize;

pub const GREEN: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
pub const YELLOW: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
pub const RED: Color = Color::Rgb(0xf3, 0x8b, 0xa8);
/// Row label column, before the bar.
const LABEL: usize = 7;
/// Countdown column, after the percent.
const COUNTDOWN: usize = 6;

pub fn health(remaining: f64) -> Color {
    if remaining >= 50.0 {
        GREEN
    } else if remaining >= 20.0 {
        YELLOW
    } else {
        RED
    }
}

/// `quota · 14:02 · refreshing…`
pub fn title(snapshot: Option<&Snapshot>, refreshing: bool) -> String {
    let mut parts = vec!["quota".to_string()];
    if let Some(snapshot) = snapshot {
        parts.push(
            snapshot
                .updated_at
                .with_timezone(&Local)
                .format("%H:%M")
                .to_string(),
        );
    }
    if refreshing {
        parts.push("refreshing…".into());
    }
    parts.join(" · ")
}

/// Two units, no space: `4d21h`, `4h38m`, `12m`, `<1m`, `now`.
pub fn countdown(seconds: i64) -> String {
    if seconds <= 0 {
        return "now".into();
    }
    let (days, hours, minutes) = (
        seconds / 86_400,
        seconds % 86_400 / 3600,
        seconds % 3600 / 60,
    );
    if days > 0 {
        let both = format!("{days}d{hours}h");
        if both.len() <= COUNTDOWN {
            both
        } else {
            format!("{days}d")
        }
    } else if hours > 0 {
        format!("{hours}h{minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        "<1m".into()
    }
}

fn until(at: Time, now: Time) -> String {
    countdown((at - now).num_seconds())
}

/// Fill is what is left at half-cell resolution; `┃` marks where linear use would be now (time
/// left in the cycle), and is left out when the pace is unknown.
pub fn bar(remaining: Option<f64>, marker: Option<f64>, width: usize) -> Vec<Span<'static>> {
    let fill = remaining.map_or(Style::default().fg(SURFACE2), |r| {
        Style::default().fg(health(r))
    });
    let mut halves = 0usize;
    if let Some(remaining) = remaining {
        let pct = remaining.clamp(0.0, 100.0);
        halves = (pct / 100.0 * width as f64 * 2.0).round() as usize;
        if pct > 0.0 && halves == 0 {
            halves = 1;
        }
        if pct < 100.0 && halves == width * 2 {
            halves = width * 2 - 1;
        }
    }
    let mut cells: Vec<(char, Style)> = (0..width)
        .map(|cell| match halves.saturating_sub(cell * 2).min(2) {
            2 => ('━', fill),
            1 => ('╸', fill),
            _ => ('─', Style::default().fg(SURFACE2)),
        })
        .collect();
    if let Some(marker) = marker.filter(|m| m.is_finite())
        && width > 0
    {
        let cell = ((marker / 100.0 * width as f64).round().max(0.0) as usize).min(width - 1);
        cells[cell] = ('┃', Style::default().fg(TEXT).add_modifier(Modifier::BOLD));
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (glyph, style) in cells {
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push(glyph),
            _ => spans.push(Span::styled(glyph.to_string(), style)),
        }
    }
    spans
}

fn verdict(runway: Runway, now: Time) -> Vec<Span<'static>> {
    match runway {
        Runway::OnPace => vec![
            Span::styled("on pace ", OVERLAY),
            Span::styled("✓", Style::default().fg(GREEN).bold()),
        ],
        Runway::ExhaustedNow => vec![Span::styled(
            "✗ exhausted now",
            Style::default().fg(RED).bold(),
        )],
        Runway::EmptyAt(at) => vec![Span::styled(
            format!("empty in {}", until(at, now)),
            Style::default().fg(YELLOW).bold(),
        )],
        Runway::Unknown => vec![Span::styled("runway unknown", OVERLAY)],
    }
}

fn width_of(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| s.content.chars().count()).sum()
}

/// `left` and `right` on one line, `right` flush with `width`. The left side gives way.
fn spread(mut left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let room = width.saturating_sub(width_of(&right) + 1);
    while width_of(&left) > room {
        let over = width_of(&left) - room;
        let Some(last) = left.last_mut() else { break };
        let chars = last.content.chars().count();
        if chars <= over {
            left.pop();
        } else {
            last.content = truncate(&last.content, chars - over).into();
        }
    }
    if right.is_empty() {
        return Line::from(left);
    }
    let gap = width.saturating_sub(width_of(&left) + width_of(&right));
    left.push(Span::raw(" ".repeat(gap)));
    left.extend(right);
    Line::from(left)
}

fn text_line(text: String, style: impl Into<Style>, width: usize) -> Line<'static> {
    Line::styled(truncate(&text, width), style)
}

/// Windows grouped as the provider reported them, keeping their order.
fn groups<'a>(windows: &[&'a Window]) -> Vec<(Option<&'a str>, Vec<&'a Window>)> {
    let mut groups: Vec<(Option<&str>, Vec<&Window>)> = Vec::new();
    for window in windows {
        let key = window.group.as_deref();
        match groups.iter_mut().find(|(g, _)| *g == key) {
            Some((_, members)) => members.push(window),
            None => groups.push((key, vec![window])),
        }
    }
    groups
}

/// The headline percent and its window, the bar, then one row per window.
fn group_lines(
    windows: &[&Window],
    headline: &Headline<'_>,
    now: Time,
    stale: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let marker = headline
        .limiting
        .and_then(|w| pace::pace(w, now, stale))
        .map(|p| p.time_remaining_percent);
    lines.push(match (headline.remaining, headline.limiting) {
        (Some(remaining), Some(limiting)) => Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{:.0}%", remaining),
                Style::default().fg(health(remaining)).bold(),
            ),
            Span::styled(
                truncate(&format!(" {}", limiting.label), width.saturating_sub(7)),
                OVERLAY,
            ),
        ]),
        _ => text_line("  effective unknown".into(), OVERLAY, width),
    });
    let mut bar_line = vec![Span::raw("  ")];
    bar_line.extend(bar(
        headline.remaining,
        marker,
        width.saturating_sub(2).max(3),
    ));
    lines.push(Line::from(bar_line));
    let row_bar = width
        .saturating_sub(2 + LABEL + 1 + 1 + 4 + 1 + COUNTDOWN)
        .max(3);
    for window in windows {
        let remaining = window.remaining();
        let marker = pace::pace(window, now, stale).map(|p| p.time_remaining_percent);
        let mut row = vec![Span::styled(
            format!("  {:<LABEL$} ", truncate(&window.label, LABEL)),
            SUBTEXT,
        )];
        row.extend(bar(remaining, marker, row_bar));
        row.push(match remaining {
            Some(r) => Span::styled(
                format!(" {:>4}", format!("{r:.0}%")),
                Style::default().fg(health(r)),
            ),
            None => Span::styled(format!(" {:>4}", "?"), OVERLAY),
        });
        row.push(Span::styled(
            format!(
                " {}",
                window.resets_at.map_or(String::new(), |at| until(at, now))
            ),
            OVERLAY,
        ));
        lines.push(Line::from(row));
    }
    lines
}

/// One provider's card. `in_view` words the Keychain remedy for the sidebar view's Enter key.
pub fn card(provider: &Provider, now: Time, width: usize, in_view: bool) -> Vec<Line<'static>> {
    let name = format!("{:<6}", provider.provider.as_str());
    let failed = |icon_style: Style, message: &str, remedy: Option<&str>| {
        let mut lines = vec![spread(
            vec![
                Span::styled("○ ", icon_style),
                Span::styled(format!("{name} "), Style::default().fg(TEXT).bold()),
                Span::styled(message.to_string(), icon_style),
            ],
            vec![],
            width,
        )];
        if let Some(remedy) = remedy {
            lines.push(text_line(format!("  {remedy}"), OVERLAY, width));
        }
        lines
    };
    let error = provider.error.as_deref().unwrap_or("failed");
    match provider.status {
        Status::NotSetUp => {
            return failed(
                Style::default().fg(OVERLAY),
                "not set up",
                provider.remedy.as_deref(),
            );
        }
        Status::NeedsKeychain => {
            let remedy = if in_view {
                Some("enter allows")
            } else {
                provider.remedy.as_deref()
            };
            let mut lines = failed(Style::default().fg(YELLOW), "needs Keychain access", remedy);
            if error != "needs Keychain access" {
                lines.insert(1, text_line(format!("  {error}"), OVERLAY, width));
            }
            return lines;
        }
        Status::Failed => {
            return failed(Style::default().fg(RED), error, provider.remedy.as_deref());
        }
        Status::Ok | Status::Stale => {}
    }
    let (windows, dropped) = live(provider, now);
    let stale = provider.status == Status::Stale || dropped;
    let groups = groups(&windows);
    let single = matches!(groups.as_slice(), [(None, _)]);
    let icon = if stale {
        Span::styled("◐ ", Style::default().fg(YELLOW))
    } else {
        Span::styled("● ", Style::default().fg(GREEN))
    };
    let left = vec![
        icon,
        Span::styled(format!("{name} "), Style::default().fg(TEXT).bold()),
        Span::styled(provider.plan.clone().unwrap_or_default(), SUBTEXT),
    ];
    let headlines: Vec<_> = groups
        .iter()
        .map(|(_, members)| pace::headline(members, now, stale))
        .collect();
    let right = if stale {
        vec![Span::styled("stale", Style::default().fg(YELLOW))]
    } else if single {
        verdict(headlines[0].runway, now)
    } else {
        vec![]
    };
    let mut lines = vec![spread(left, right, width)];
    if stale {
        let age = provider
            .fetched_at
            .filter(|at| *at <= now)
            .map_or("unknown".into(), |at| until(now, at).replace("now", "<1m"));
        let why = match &provider.error {
            Some(error) => format!("  {error} · read {age} ago"),
            None => format!("  a window reset since · read {age} ago"),
        };
        lines.push(text_line(why, OVERLAY, width));
    }
    if windows.is_empty() {
        lines.push(text_line(
            "  every window has reset · r reads".into(),
            OVERLAY,
            width,
        ));
    }
    for ((group, members), headline) in groups.iter().zip(&headlines) {
        if let Some(group) = group {
            let right = if stale {
                vec![]
            } else {
                verdict(headline.runway, now)
            };
            lines.push(spread(
                vec![Span::styled(
                    format!("  {group}"),
                    Style::default().fg(TEXT),
                )],
                right,
                width,
            ));
        }
        lines.extend(group_lines(members, headline, now, stale, width));
    }
    lines
}

/// An expired login means this plan is currently unused.
fn visible(provider: &super::Provider) -> bool {
    !provider.error.as_deref().is_some_and(|error| {
        let error = error.to_ascii_lowercase();
        error.contains("expired")
            && ["login", "token", "session", "credential"]
                .iter()
                .any(|word| error.contains(word))
    })
}

/// Every card, a blank line apart.
pub fn cards(snapshot: &Snapshot, now: Time, width: usize, in_view: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for provider in snapshot.providers.iter().filter(|p| visible(p)) {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.extend(card(provider, now, width, in_view));
    }
    lines
}

/// The sidebar's quota panel: provider headings with their windows nested below.
pub fn summary(snapshot: &Snapshot, now: Time, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for provider in snapshot
        .providers
        .iter()
        .filter(|p| p.status != Status::NotSetUp && visible(p))
    {
        let name = Span::styled(provider.provider.as_str(), Style::default().fg(TEXT).bold());
        let problem = |style: Style, text: &str| {
            Line::from(vec![
                Span::styled("○ ", style),
                name.clone(),
                Span::styled(
                    truncate(&format!(" {text}"), width.saturating_sub(2 + name.width())),
                    style,
                ),
            ])
        };
        match provider.status {
            Status::NeedsKeychain => {
                lines.push(problem(Style::default().fg(YELLOW), "needs Keychain · u"));
                continue;
            }
            Status::Failed => {
                lines.push(problem(
                    Style::default().fg(RED),
                    provider.error.as_deref().unwrap_or("failed"),
                ));
                continue;
            }
            Status::Ok | Status::Stale | Status::NotSetUp => {}
        }
        let (windows, dropped) = live(provider, now);
        let stale = provider.status == Status::Stale || dropped;
        let icon = if stale {
            Span::styled("◐ ", Style::default().fg(YELLOW))
        } else {
            Span::styled("● ", Style::default().fg(GREEN))
        };
        lines.push(Line::from(vec![icon, name]));
        let windows: Vec<_> = windows
            .into_iter()
            .filter(|w| w.role != super::Role::Model)
            .collect();
        if windows.is_empty() {
            lines.push(text_line("  no current windows".into(), OVERLAY, width));
        }
        let group_width = windows
            .iter()
            .filter_map(|w| w.group.as_ref())
            .map(|g| g.chars().count())
            .max()
            .unwrap_or(0)
            .min(width.saturating_sub(2 + 1 + 4 + 1 + 3 + 5 + 1 + COUNTDOWN));
        for (group, windows) in groups(&windows) {
            let label_width = if group.is_some() { 4 } else { LABEL };
            for (row, window) in windows.into_iter().enumerate() {
                let label = match window.window_seconds {
                    Some(18_000) => "5h",
                    Some(604_800) => "week",
                    _ => &window.label,
                };
                let mut spans = vec![Span::raw("  ")];
                if let Some(group) = group {
                    spans.push(Span::styled(
                        format!(
                            "{:<group_width$} ",
                            if row == 0 {
                                truncate(group, group_width)
                            } else {
                                String::new()
                            }
                        ),
                        SUBTEXT,
                    ));
                }
                spans.push(Span::styled(
                    format!("{:<label_width$} ", truncate(label, label_width)),
                    OVERLAY,
                ));
                let remaining = window.remaining();
                let marker = pace::pace(window, now, stale).map(|p| p.time_remaining_percent);
                spans.extend(bar(
                    remaining,
                    marker,
                    width
                        .saturating_sub(width_of(&spans) + 5 + 1 + COUNTDOWN)
                        .max(3),
                ));
                spans.push(match remaining {
                    Some(r) => Span::styled(
                        format!(" {:>4}", format!("{r:.0}%")),
                        Style::default().fg(health(r)),
                    ),
                    None => Span::styled(format!(" {:>4}", "?"), OVERLAY),
                });
                let reset = window.resets_at.map_or(String::new(), |at| until(at, now));
                spans.push(Span::styled(format!(" {reset:<COUNTDOWN$}"), OVERLAY));
                lines.push(Line::from(spans));
            }
        }
    }
    lines
}

/// The `quota` command's text: the title, then the cards without colour.
pub fn text(snapshot: &Snapshot, now: Time, width: usize) -> Vec<String> {
    let mut out = vec![title(Some(snapshot), false), String::new()];
    out.extend(cards(snapshot, now, width, false).into_iter().map(|line| {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
            .trim_end()
            .to_string()
    }));
    out
}

/// `quota --json`: the snapshot with each group's headline, runway and each window's pace,
/// computed for `generated_at`.
#[derive(Serialize)]
pub struct Report {
    pub generated_at: Time,
    pub updated_at: Time,
    pub providers: Vec<ProviderReport>,
}

#[derive(Serialize)]
pub struct ProviderReport {
    pub provider: super::ProviderId,
    pub status: Status,
    pub stale: bool,
    pub plan: Option<String>,
    pub fetched_at: Option<Time>,
    pub error: Option<String>,
    pub remedy: Option<String>,
    pub groups: Vec<GroupReport>,
}

#[derive(Serialize)]
pub struct GroupReport {
    pub group: Option<String>,
    /// The least left over the group's bounding windows.
    pub percent_remaining: Option<f64>,
    pub limiting_window: Option<String>,
    pub runway: &'static str,
    pub empty_at: Option<Time>,
    pub windows: Vec<WindowReport>,
}

#[derive(Serialize)]
pub struct WindowReport {
    #[serde(flatten)]
    pub window: Window,
    pub percent_remaining: Option<f64>,
    pub pace: Option<pace::Pace>,
}

pub fn report(snapshot: &Snapshot, now: Time) -> Report {
    Report {
        generated_at: now,
        updated_at: snapshot.updated_at,
        providers: snapshot
            .providers
            .iter()
            .map(|provider| {
                let (windows, dropped) = live(provider, now);
                let stale = provider.status == Status::Stale || dropped;
                ProviderReport {
                    provider: provider.provider,
                    status: provider.status,
                    stale,
                    plan: provider.plan.clone(),
                    fetched_at: provider.fetched_at,
                    error: provider.error.clone(),
                    remedy: provider.remedy.clone(),
                    groups: groups(&windows)
                        .into_iter()
                        .map(|(group, members)| {
                            let headline = pace::headline(&members, now, stale);
                            GroupReport {
                                group: group.map(str::to_string),
                                percent_remaining: headline.remaining,
                                limiting_window: headline.limiting.map(|w| w.id.clone()),
                                runway: headline.runway.as_str(),
                                empty_at: match headline.runway {
                                    Runway::EmptyAt(at) => Some(at),
                                    _ => None,
                                },
                                windows: members
                                    .into_iter()
                                    .map(|w| WindowReport {
                                        window: w.clone(),
                                        percent_remaining: w.remaining(),
                                        pace: pace::pace(w, now, stale),
                                    })
                                    .collect(),
                            }
                        })
                        .collect(),
                }
            })
            .collect(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::quota::tests::at;
    use crate::quota::{ProviderId, Role};

    const HOUR: i64 = 3600;
    pub const NOW: i64 = 1_794_000_000;

    fn w(id: &str, label: &str, used: f64, resets_in: i64, seconds: u64) -> Window {
        Window {
            id: id.into(),
            label: label.into(),
            group: None,
            role: Role::Bound,
            percent_used: Some(used),
            resets_at: Some(at(NOW + resets_in)),
            starts_at: None,
            window_seconds: Some(seconds),
        }
    }

    fn provider(id: ProviderId, status: Status, windows: Vec<Window>) -> Provider {
        Provider {
            provider: id,
            status,
            plan: None,
            windows,
            fetched_at: Some(at(NOW - 60)),
            error: None,
            remedy: None,
        }
    }

    /// One of each kind of card.
    pub fn sample() -> Snapshot {
        let mut claude = provider(
            ProviderId::Claude,
            Status::Ok,
            vec![
                w("five_hour", "session", 3.0, 4 * HOUR + 38 * 60, 18_000),
                w("seven_day", "week", 28.0, 4 * 86_400 + 21 * HOUR, 604_800),
            ],
        );
        claude.plan = Some("max".into());
        let mut codex = provider(
            ProviderId::Codex,
            Status::Stale,
            vec![
                w("five_hour", "5h", 40.0, 2 * HOUR, 18_000),
                w("weekly", "week", 17.0, 3 * 86_400, 604_800),
            ],
        );
        codex.plan = Some("pro".into());
        codex.error = Some("reading rate limits: timed out".into());
        let mut gemini = w("gemini_5h", "5h", 25.0, 2 * HOUR, 18_000);
        gemini.group = Some("gemini".into());
        let mut gpt = w("claude_gpt_5h", "5h", 90.0, HOUR, 18_000);
        gpt.group = Some("claude/gpt".into());
        let agy = provider(ProviderId::Agy, Status::Ok, vec![gemini, gpt]);
        let mut grok = provider(ProviderId::Grok, Status::Failed, vec![]);
        grok.error = Some("login expired".into());
        grok.remedy = Some("run grok to renew it".into());
        Snapshot {
            updated_at: at(NOW),
            providers: vec![claude, codex, agy, grok],
        }
    }

    fn plain(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn countdowns() {
        assert_eq!(countdown(4 * 3600 + 38 * 60), "4h38m");
        assert_eq!(countdown(4 * 86_400 + 21 * 3600), "4d21h");
        assert_eq!(countdown(120 * 86_400 + 3600), "120d1h");
        assert_eq!(countdown(1200 * 86_400), "1200d");
        assert_eq!(countdown(12 * 60), "12m");
        assert_eq!(countdown(30), "<1m");
        assert_eq!(countdown(0), "now");
    }

    #[test]
    fn bar_fills_halves_and_marks_pace() {
        let text = |spans: Vec<Span<'_>>| {
            spans
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>()
        };
        assert_eq!(text(bar(Some(50.0), None, 10)), "━━━━━─────");
        assert_eq!(text(bar(Some(55.0), None, 10)), "━━━━━╸────");
        assert_eq!(
            text(bar(Some(0.5), None, 10)),
            "╸─────────",
            "any left shows"
        );
        assert_eq!(
            text(bar(Some(99.9), None, 10)),
            "━━━━━━━━━╸",
            "full only at 100"
        );
        assert_eq!(text(bar(Some(50.0), Some(80.0), 10)), "━━━━━───┃─");
        assert_eq!(text(bar(None, Some(100.0), 4)), "───┃");
        assert_eq!(bar(Some(80.0), None, 10)[0].style.fg, Some(GREEN));
        assert_eq!(bar(Some(30.0), None, 10)[0].style.fg, Some(YELLOW));
        assert_eq!(bar(Some(10.0), None, 10)[0].style.fg, Some(RED));
    }

    #[test]
    fn cards_fit_30_40_and_60_columns() {
        let snapshot = sample();
        for width in [30, 40, 60] {
            let lines = plain(&cards(&snapshot, at(NOW), width, true));
            for line in &lines {
                assert!(line.chars().count() <= width, "{width}: {line:?}");
            }
            let all = lines.join("\n");
            assert!(all.contains("97%"), "claude session left");
            assert!(all.contains("4h38m") && all.contains("4d21h"));
            assert!(
                all.contains("72% week"),
                "headline names the limiting window"
            );
            assert!(all.contains("◐ codex"));
            assert!(all.contains("stale"));
            assert!(all.contains("gemini") && all.contains("claude/gpt"));
            assert!(!all.contains("○ grok"));
            assert!(!all.contains("run grok to renew it"));
        }
        let wide = plain(&cards(&snapshot, at(NOW), 40, true));
        assert_eq!(
            wide[0].trim_end(),
            format!("● claude max{}on pace ✓", " ".repeat(40 - 12 - 9))
        );
        assert!(
            wide.iter()
                .any(|l| l.starts_with("  session ━") && l.ends_with(" 97% 4h38m"))
        );
        assert!(
            wide.iter()
                .any(|l| l.contains("claude/gpt") && l.contains("empty in"))
        );
    }

    #[test]
    fn keychain_card_words_the_remedy_for_where_it_is_shown() {
        let mut claude = provider(ProviderId::Claude, Status::NeedsKeychain, vec![]);
        claude.error = Some("needs Keychain access".into());
        claude.remedy = Some(crate::quota::claude::ALLOW_REMEDY.into());
        let view = plain(&card(&claude, at(NOW), 40, true));
        assert_eq!(view, ["○ claude needs Keychain access", "  enter allows"]);
        let cli = plain(&card(&claude, at(NOW), 60, false));
        assert_eq!(cli[1], format!("  {}", crate::quota::claude::ALLOW_REMEDY));
    }

    #[test]
    fn a_window_past_its_reset_is_dropped_and_the_card_goes_stale() {
        let claude = provider(
            ProviderId::Claude,
            Status::Ok,
            vec![
                w("five_hour", "session", 80.0, -60, 18_000),
                w("seven_day", "week", 10.0, 86_400, 604_800),
            ],
        );
        let lines = plain(&card(&claude, at(NOW), 40, true)).join("\n");
        assert!(!lines.contains("20%"));
        assert!(lines.contains("◐ claude") && lines.contains("a window reset since"));
    }

    #[test]
    fn json_report_carries_headline_runway_and_pace() {
        let report = serde_json::to_value(report(&sample(), at(NOW))).unwrap();
        let claude = &report["providers"][0];
        assert_eq!(claude["groups"][0]["percent_remaining"], 72.0);
        assert_eq!(claude["groups"][0]["limiting_window"], "seven_day");
        assert_eq!(claude["groups"][0]["runway"], "on_pace");
        assert_eq!(claude["groups"][0]["windows"][0]["percent_remaining"], 97.0);
        assert!(claude["groups"][0]["windows"][0]["pace"]["time_remaining_percent"].is_number());
        let codex = &report["providers"][1];
        assert_eq!(codex["stale"], true);
        assert!(codex["groups"][0]["windows"][0]["pace"].is_null());
        let agy = &report["providers"][2];
        assert_eq!(agy["groups"][1]["group"], "claude/gpt");
        assert_eq!(agy["groups"][1]["runway"], "empty_in");
        assert_eq!(report["providers"][3]["status"], "failed");
        assert!(!serde_json::to_string(&report).unwrap().contains("Bearer"));
    }

    #[test]
    fn title_shows_refreshing() {
        assert_eq!(title(None, true), "quota · refreshing…");
        assert!(title(Some(&sample()), false).starts_with("quota · "));
    }

    #[test]
    fn summary_nests_all_windows_under_one_provider_heading() {
        let mut snapshot = sample();
        let mut week = snapshot.providers[2].windows[0].clone();
        week.label = "week".into();
        week.window_seconds = Some(604_800);
        snapshot.providers[2].windows.insert(1, week);
        let lines = plain(&summary(&snapshot, at(NOW), 36));
        assert_eq!(lines.len(), 10, "{lines:#?}");
        assert_eq!(lines[0], "● claude");
        assert_eq!(lines[3], "◐ codex");
        assert_eq!(lines[6], "● agy");
        assert!(lines[7].starts_with("  gemini     5h"));
        assert!(lines[8].starts_with("             week"));
        assert!(lines[9].starts_with("  claude/gpt 5h"));
        for (row, label, percent) in [
            (1, "5h", "97%"),
            (2, "week", "72%"),
            (4, "5h", "60%"),
            (5, "week", "83%"),
            (8, "week", "75%"),
        ] {
            assert!(
                lines[row].trim_start().starts_with(label) && lines[row].contains(percent),
                "{}",
                lines[row]
            );
            assert_eq!(lines[row].chars().count(), 36);
        }
        assert!(lines[1].contains('┃') && lines[2].contains('┃'));
        assert!(!lines[4].contains('┃') && !lines[5].contains('┃'));
        assert!(lines[7].contains('┃') && lines[9].contains('┃'));
        assert!(lines[7].contains("75%") && lines[9].contains("10%"));
        assert!(!lines.iter().any(|line| line.contains("expired")));
    }

    #[test]
    fn summary_keeps_only_live_windows_and_omits_model_limits() {
        let mut snapshot = sample();
        let mut model = snapshot.providers[0].windows[1].clone();
        model.role = Role::Model;
        model.label = "opus".into();
        snapshot.providers[0].windows.push(model);
        snapshot.providers[0].windows[0].resets_at = Some(at(NOW));
        snapshot.providers[1].status = Status::Ok;
        snapshot.providers[1].windows[1].percent_used = None;
        for width in [30, 36, 58] {
            let rows = plain(&summary(&snapshot, at(NOW), width));
            assert_eq!(rows.len(), 8);
            assert!(rows.iter().all(|row| row.chars().count() <= width));
            assert_eq!(rows[0], "◐ claude");
            assert!(rows[1].trim_start().starts_with("week") && !rows[1].contains('┃'));
            assert!(rows[3].trim_start().starts_with("5h") && rows[3].contains('┃'));
            assert!(rows[4].trim_start().starts_with("week") && rows[4].contains('?'));
            assert!(!rows.iter().any(|row| row.contains("opus")));
        }
        snapshot.providers[0].status = Status::Failed;
        snapshot.providers[0].error = Some("offline".into());
        let rows = plain(&summary(&snapshot, at(NOW), 36));
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[0], "○ claude offline");
        snapshot.providers[0].status = Status::Ok;
        snapshot.providers[0].windows.clear();
        let rows = plain(&summary(&snapshot, at(NOW), 36));
        assert_eq!(rows[1], "  no current windows");
    }
}
