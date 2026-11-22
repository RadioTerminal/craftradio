//! Program as Opus packets on a data channel, with signed blocks of their hashes.
//!
//! The page opens the [`AUDIO_CHANNEL`] (unordered, no retransmits). Each message is one
//! 20 ms Opus packet with a sequence number. Every [`BLOCK_PACKETS`] packets, the station
//! sends a block on the reliable "now" channel: the hash of each packet, signed with the
//! station key. A page checks each packet against its block before it plays it. Thus a
//! listener that passes the audio on cannot change it.
//!
//! All integers are big-endian.
//!
//! Audio message: kind ([`KIND_AUDIO`]), `seq` (u32), `hops` (u8, 0 from the station), the
//! Opus packet.
//!
//! Block message: kind ([`KIND_BLOCK`]), `first_seq` (u32), `count` (u8), `count` hashes of
//! [`HASH_LEN`] bytes, then a 64-byte Ed25519 signature over [`BLOCK_CONTEXT`], the station
//! ID (its 43 characters) and every byte before the signature. A station without a key
//! sends 64 zero bytes in place of the signature.

use crate::relay::Identity;

/// The label of the data channel that carries the audio.
pub const AUDIO_CHANNEL: &str = "audio";
pub const KIND_AUDIO: u8 = 1;
pub const KIND_BLOCK: u8 = 2;
/// Packets per block: 200 ms.
pub const BLOCK_PACKETS: usize = 10;
/// The first bytes of SHA-256 over `seq` and the packet.
pub const HASH_LEN: usize = 16;
/// The signature covers this text first, so it cannot pass for a signature of anything else.
pub const BLOCK_CONTEXT: &[u8] = b"craftradio-block\n";
pub const SIGNATURE_LEN: usize = 64;

/// The hash of one packet in a block.
pub fn packet_hash(seq: u32, opus: &[u8]) -> [u8; HASH_LEN] {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    ctx.update(&seq.to_be_bytes());
    ctx.update(opus);
    let digest = ctx.finish();
    let mut out = [0; HASH_LEN];
    out.copy_from_slice(&digest.as_ref()[..HASH_LEN]);
    out
}

/// Numbers the packets and makes a block every [`BLOCK_PACKETS`].
pub(crate) struct Blocks {
    identity: Option<Identity>,
    id: String,
    next_seq: u32,
    hashes: Vec<[u8; HASH_LEN]>,
}

impl Blocks {
    pub(crate) fn new(identity: Option<Identity>) -> Self {
        let id = identity.as_ref().map(Identity::id).unwrap_or_default();
        Self {
            identity,
            id,
            next_seq: 0,
            hashes: Vec::with_capacity(BLOCK_PACKETS),
        }
    }

    /// The audio message for `opus`, and the block message when this packet ends a block.
    pub(crate) fn push(&mut self, opus: &[u8]) -> (Vec<u8>, Option<Vec<u8>>) {
        let seq = self.next_seq;
        self.next_seq = seq.wrapping_add(1);
        let mut audio = Vec::with_capacity(6 + opus.len());
        audio.push(KIND_AUDIO);
        audio.extend_from_slice(&seq.to_be_bytes());
        audio.push(0);
        audio.extend_from_slice(opus);
        self.hashes.push(packet_hash(seq, opus));
        if self.hashes.len() < BLOCK_PACKETS {
            return (audio, None);
        }
        let count = self.hashes.len();
        let first = seq.wrapping_sub(count as u32 - 1);
        let mut block = Vec::with_capacity(6 + count * HASH_LEN + SIGNATURE_LEN);
        block.push(KIND_BLOCK);
        block.extend_from_slice(&first.to_be_bytes());
        block.push(count as u8);
        for h in self.hashes.drain(..) {
            block.extend_from_slice(&h);
        }
        match &self.identity {
            Some(identity) => {
                let mut message =
                    Vec::with_capacity(BLOCK_CONTEXT.len() + self.id.len() + block.len());
                message.extend_from_slice(BLOCK_CONTEXT);
                message.extend_from_slice(self.id.as_bytes());
                message.extend_from_slice(&block);
                block.extend_from_slice(&identity.sign(&message));
            }
            None => block.extend_from_slice(&[0; SIGNATURE_LEN]),
        }
        (audio, Some(block))
    }
}
