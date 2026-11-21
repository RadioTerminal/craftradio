//! Commands to the render thread and events from it.

use std::time::Duration;

/// One of the four decks.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DeckId {
    /// Deck A.
    A,
    /// Deck B.
    B,
    /// Deck C.
    C,
    /// Deck D.
    D,
}

impl DeckId {
    /// All decks, in order.
    pub const ALL: [DeckId; 4] = [DeckId::A, DeckId::B, DeckId::C, DeckId::D];

    /// Returns the position of the deck in [`DeckId::ALL`].
    pub fn index(self) -> usize {
        match self {
            DeckId::A => 0,
            DeckId::B => 1,
            DeckId::C => 2,
            DeckId::D => 3,
        }
    }
}

/// A mixer strip with a fader and a meter.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Strip {
    /// A deck.
    Deck(DeckId),
    /// The cart bus.
    Cart,
    /// The live input bus.
    Live,
    /// Program out, after the limiter.
    Out,
}

impl Strip {
    /// The number of strips.
    pub const COUNT: usize = 7;

    /// All strips, in meter order.
    pub const ALL: [Strip; Strip::COUNT] = [
        Strip::Deck(DeckId::A),
        Strip::Deck(DeckId::B),
        Strip::Deck(DeckId::C),
        Strip::Deck(DeckId::D),
        Strip::Cart,
        Strip::Live,
        Strip::Out,
    ];

    /// Returns the position of the strip in [`Strip::ALL`].
    pub fn index(self) -> usize {
        match self {
            Strip::Deck(deck) => deck.index(),
            Strip::Cart => 4,
            Strip::Live => 5,
            Strip::Out => 6,
        }
    }
}

/// A command that the mixer applies at an exact sample.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Command {
    /// The absolute frame count at which the command takes effect. The count starts at 0
    /// with the mixer. A late command takes effect at the start of the next render block.
    pub at_sample: u64,
    /// What to do.
    pub kind: CommandKind,
    /// The tag of the deck item that this command is for, from [`crate::LoadOptions`]. A
    /// deck command with a tag has no effect when the deck holds a different item.
    pub item_tag: Option<u64>,
}

impl Command {
    /// Makes a command with no item tag.
    pub fn new(at_sample: u64, kind: CommandKind) -> Self {
        Self {
            at_sample,
            kind,
            item_tag: None,
        }
    }

    /// Limits the command to the deck item with this tag.
    pub fn for_item(mut self, item_tag: u64) -> Self {
        self.item_tag = Some(item_tag);
        self
    }
}

/// What a [`Command`] does.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum CommandKind {
    /// Starts a loaded deck, or continues a paused deck.
    Play(DeckId),
    /// Pauses a playing deck. `Play` continues it.
    Pause(DeckId),
    /// Stops the deck at once and unloads its item. The deck becomes empty.
    Stop(DeckId),
    /// Fades the deck to silence over `over`, then unloads it and sends `DeckEnded`. A late
    /// fade still ends at `at_sample + over`.
    FadeOut {
        /// The deck.
        deck: DeckId,
        /// The length of the fade.
        over: Duration,
    },
    /// Sets the fader of a strip, in dB. The change ramps over 5 ms. The handle refuses NaN
    /// and +inf, takes -inf as mute, and limits the gain to +12 dB.
    SetGain {
        /// The strip.
        strip: Strip,
        /// The gain in dB. 0 is unity.
        db: f32,
    },
    /// Plays the clip of a cart slot from its start on a free cart voice.
    FireCart {
        /// The cartwall slot.
        slot: usize,
    },
    /// Stops all cart voices at once.
    StopCarts,
    /// Lowers decks and live input by `db` over `attack`. Carts mix in after the duck.
    Duck {
        /// The duck depth in dB, for example −12. The handle refuses NaN and +inf.
        db: f32,
        /// The time to reach the full depth.
        attack: Duration,
        /// The time that a later `Unduck` takes to return to 0 dB.
        release: Duration,
    },
    /// Returns from a duck to 0 dB over the release time of the last `Duck`.
    Unduck,
}

/// An event from the render thread.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AudioEvent {
    /// The mixer took the new item for the deck. `item_tag` is the tag of that load.
    Loaded {
        /// The deck.
        deck: DeckId,
        /// The tag from [`crate::LoadOptions`].
        item_tag: Option<u64>,
    },
    /// A `Play` started the item on the deck (not a continue after `Pause`). `at_sample` is
    /// the frame where its first sample sounds; `item_tag` is the tag of its load.
    Started {
        /// The deck.
        deck: DeckId,
        /// The first frame of the item on Program.
        at_sample: u64,
        /// The tag from [`crate::LoadOptions`].
        item_tag: Option<u64>,
    },
    /// The mixer refused the new item, because the deck was on air. The deck keeps its
    /// item. `item_tag` is the tag of the refused load.
    LoadRejected {
        /// The deck.
        deck: DeckId,
        /// The tag from [`crate::LoadOptions`].
        item_tag: Option<u64>,
    },
    /// A `FadeOut` began on a playing deck. `DeckEnded` follows at its end.
    Fading {
        /// The deck.
        deck: DeckId,
        /// The tag of the item that fades.
        item_tag: Option<u64>,
    },
    /// The item on the deck came to its end, or its fade-out finished. The deck is empty.
    DeckEnded(DeckId),
    /// The ring of a playing deck became empty before the end of the file. The deck plays
    /// silence until data comes again. The mixer sends this once per underrun.
    Underrun(DeckId),
    /// Program was silent for the time that the detector sets.
    Silence {
        /// The first sample of the silence.
        since_sample: u64,
    },
    /// Program has audio again after a `Silence` event.
    SilenceEnded {
        /// The first sample with audio.
        at_sample: u64,
    },
}
