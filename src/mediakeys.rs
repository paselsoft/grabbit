// SPDX-License-Identifier: GPL-3.0-or-later
//! Tasti multimediali del Mac (play/pausa, avanti, indietro: F7, F8, F9 sulla tastiera Apple),
//! anche con Grabbit in secondo piano.
//!
//! Da macOS 10.12.2 i tasti vanno all'app che dice al sistema di stare riproducendo
//! (`MPNowPlayingInfoCenter`) e che ascolta i comandi (`MPRemoteCommandCenter`): così non si
//! apre iTunes. Fuori dal Mac questo modulo non fa niente.

// fuori dal Mac i tasti non arrivano mai: il codice serve solo ai test
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

/// Un tasto premuto.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Key {
    Toggle,
    Play,
    Pause,
    Next,
    Prev,
}

/// Cosa si sta ascoltando, per il sistema (e per far arrivare i tasti a Grabbit).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// secondi
    pub duration: f64,
    pub elapsed: f64,
    pub playing: bool,
}

impl NowPlaying {
    /// Basta aggiornare il sistema quando cambia il brano o si mette in pausa: la posizione
    /// la calcola lui dalla velocità di riproduzione.
    fn same_state(&self, other: &NowPlaying) -> bool {
        self.title == other.title
            && self.artist == other.artist
            && self.album == other.album
            && self.playing == other.playing
    }
}

pub use imp::MediaKeys;

#[cfg(target_os = "macos")]
mod imp {
    use super::{Key, NowPlaying};
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSDictionary, NSNumber, NSString};
    use objc2_media_player::{
        MPMediaItemPropertyAlbumTitle, MPMediaItemPropertyArtist,
        MPMediaItemPropertyPlaybackDuration, MPMediaItemPropertyTitle, MPNowPlayingInfoCenter,
        MPNowPlayingInfoPropertyElapsedPlaybackTime, MPNowPlayingInfoPropertyPlaybackRate,
        MPNowPlayingPlaybackState, MPRemoteCommand, MPRemoteCommandCenter, MPRemoteCommandEvent,
        MPRemoteCommandHandlerStatus,
    };
    use std::ptr::NonNull;
    use std::sync::mpsc::{channel, Receiver};
    use std::sync::Arc;

    pub struct MediaKeys {
        rx: Receiver<Key>,
        /// i «target» restituiti dal sistema: tenuti vivi finché c'è Grabbit
        _targets: Vec<Retained<AnyObject>>,
        shown: Option<Option<NowPlaying>>,
    }

    impl MediaKeys {
        /// `wake` ridisegna la finestra quando arriva un tasto (anche a finestra nascosta).
        pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
            let (tx, rx) = channel();
            let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
            let mut targets = Vec::new();
            // SAFETY: API di MediaPlayer (macOS 10.12.2+), chiamate sul thread principale
            unsafe {
                let center = MPRemoteCommandCenter::sharedCommandCenter();
                let commands: [(Retained<MPRemoteCommand>, Key); 5] = [
                    (center.togglePlayPauseCommand(), Key::Toggle),
                    (center.playCommand(), Key::Play),
                    (center.pauseCommand(), Key::Pause),
                    (center.nextTrackCommand(), Key::Next),
                    (center.previousTrackCommand(), Key::Prev),
                ];
                for (cmd, key) in commands {
                    let (tx, wake) = (tx.clone(), wake.clone());
                    let block = RcBlock::new(move |_e: NonNull<MPRemoteCommandEvent>| {
                        let _ = tx.send(key);
                        wake();
                        MPRemoteCommandHandlerStatus::Success
                    });
                    cmd.setEnabled(true);
                    targets.push(cmd.addTargetWithHandler(&block));
                }
            }
            Self {
                rx,
                _targets: targets,
                shown: None,
            }
        }

        pub fn keys(&mut self) -> Vec<Key> {
            self.rx.try_iter().collect()
        }

        /// Dice al sistema cosa suona (o che non suona niente), solo quando cambia.
        pub fn update(&mut self, now: Option<&NowPlaying>) {
            let changed = match (&self.shown, now) {
                (Some(Some(a)), Some(b)) => !a.same_state(b),
                (Some(None), None) => false,
                _ => true,
            };
            if !changed {
                return;
            }
            self.shown = Some(now.cloned());
            // SAFETY: dizionario di NSString → NSString/NSNumber, come vuole MediaPlayer
            unsafe {
                let center = MPNowPlayingInfoCenter::defaultCenter();
                let Some(n) = now else {
                    center.setNowPlayingInfo(None);
                    center.setPlaybackState(MPNowPlayingPlaybackState::Stopped);
                    return;
                };
                fn any<T: objc2::Message>(r: Retained<T>) -> Retained<AnyObject> {
                    // SAFETY: ogni oggetto Objective-C è un AnyObject
                    unsafe { Retained::cast_unchecked(r) }
                }
                let keys: [&NSString; 6] = [
                    MPMediaItemPropertyTitle,
                    MPMediaItemPropertyArtist,
                    MPMediaItemPropertyAlbumTitle,
                    MPMediaItemPropertyPlaybackDuration,
                    MPNowPlayingInfoPropertyElapsedPlaybackTime,
                    MPNowPlayingInfoPropertyPlaybackRate,
                ];
                let values: [Retained<AnyObject>; 6] = [
                    any(NSString::from_str(&n.title)),
                    any(NSString::from_str(&n.artist)),
                    any(NSString::from_str(&n.album)),
                    any(NSNumber::new_f64(n.duration)),
                    any(NSNumber::new_f64(n.elapsed)),
                    any(NSNumber::new_f64(if n.playing { 1.0 } else { 0.0 })),
                ];
                let dict = NSDictionary::from_retained_objects(&keys, &values);
                center.setNowPlayingInfo(Some(&dict));
                center.setPlaybackState(if n.playing {
                    MPNowPlayingPlaybackState::Playing
                } else {
                    MPNowPlayingPlaybackState::Paused
                });
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{Key, NowPlaying};

    /// Fuori dal Mac i tasti multimediali non passano da qui.
    pub struct MediaKeys;

    impl MediaKeys {
        pub fn new(_wake: impl Fn() + Send + Sync + 'static) -> Self {
            MediaKeys
        }

        pub fn keys(&mut self) -> Vec<Key> {
            Vec::new()
        }

        pub fn update(&mut self, _now: Option<&NowPlaying>) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_alone_does_not_count_as_a_change() {
        let a = NowPlaying {
            title: "Amazing Grace".into(),
            artist: "Il Volo".into(),
            album: "Our Christmas".into(),
            duration: 229.0,
            elapsed: 10.0,
            playing: true,
        };
        let later = NowPlaying {
            elapsed: 75.0,
            ..a.clone()
        };
        assert!(a.same_state(&later));
        let paused = NowPlaying {
            playing: false,
            ..a.clone()
        };
        assert!(!a.same_state(&paused));
        let next = NowPlaying {
            title: "Jingle Bells".into(),
            ..a.clone()
        };
        assert!(!a.same_state(&next));
    }
}
