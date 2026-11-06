//! The "Video Fragments: N; Audio Fragments: M; ..." status line.

use tokio::sync::mpsc;

use crate::log;
use crate::types::{DataType, LogLevel, ProgressInfo};
use crate::util::format_size;

#[derive(Debug, Default, Clone, Copy)]
pub struct Totals {
    pub audio_frags: u64,
    pub video_frags: u64,
    pub bytes: u64,
    pub max_seq: Option<u64>,
    pub start_frag: u64,
}

impl Totals {
    pub fn add(&mut self, p: &ProgressInfo) {
        match p.data_type {
            DataType::Audio => self.audio_frags += 1,
            DataType::Video => self.video_frags += 1,
        }
        self.bytes += p.byte_count;
        if p.max_seq > self.max_seq {
            self.max_seq = p.max_seq;
        }
        self.start_frag = p.start_frag;
    }

    pub fn line(&self, verbose: bool) -> String {
        let mut s = format!(
            "Video Fragments: {}; Audio Fragments: {}; ",
            self.video_frags, self.audio_frags
        );
        if verbose && let Some(max) = self.max_seq {
            s.push_str(&format!(
                "Max Fragments: {}; Max Sequence: {max}; ",
                (max + 1).saturating_sub(self.start_frag)
            ));
        }
        s.push_str(&format!("Total Downloaded: {}", format_size(self.bytes)));
        s
    }
}

/// Prints a status line per progress message until the channel closes.
/// Starts from `initial` (resumed fragment counts).
pub async fn report(mut rx: mpsc::UnboundedReceiver<ProgressInfo>, initial: Totals, newlines: bool) -> Totals {
    let mut t = initial;
    let verbose = log::enabled(LogLevel::Info);
    while let Some(p) = rx.recv().await {
        t.add(&p);
        if log::level() > LogLevel::Quiet {
            if newlines {
                eprintln!("{}", t.line(verbose));
            } else {
                eprint!("\r{}\x1b[K", t.line(verbose));
            }
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_and_line() {
        let mut t = Totals::default();
        for (dt, n) in [(DataType::Audio, 100), (DataType::Video, 2048), (DataType::Video, 2048)] {
            t.add(&ProgressInfo { data_type: dt, byte_count: n, max_seq: Some(20), start_frag: 10 });
        }
        assert_eq!(t.line(false), "Video Fragments: 2; Audio Fragments: 1; Total Downloaded: 4.10KiB");
        assert_eq!(
            t.line(true),
            "Video Fragments: 2; Audio Fragments: 1; Max Fragments: 11; Max Sequence: 20; Total Downloaded: 4.10KiB"
        );
    }
}
