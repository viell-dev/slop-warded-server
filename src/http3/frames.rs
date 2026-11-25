//! Where the frames of a stream begin and end, so that h3 reads one at a time, and none above a limit.
//!
//! h3 0.0.8 holds all of a frame until it is complete, whatever length the frame declares
//! (<https://github.com/hyperium/h3/pull/366>). It also reads from the stream once for every frame
//! that it decodes, so small frames pile up when the application is slower than the client
//! (<https://github.com/hyperium/h3/pull/344>, merged but not released). Either way a client fills
//! the server's memory byte by byte, and QUIC does not stop it, because what h3 has read counts as
//! received.
//!
//! So every read by h3 gets one whole frame, or the header or a piece of one DATA frame. What h3
//! would skip is dropped here as it arrives, and a frame that it would hold is refused by its
//! declared length. The rest, such as which frames may come when, stays with h3.
//!
//! An h3 release that bounds both can replace this.

use bytes::{Buf, Bytes};
use std::mem;

/// The largest frame on a control stream. Real ones have at most tens of bytes.
const CONTROL_FRAME_BYTES: u64 = 16 * 1024;

/// The type of a control stream (RFC 9114 section 6.2.1).
const CONTROL_STREAM: u64 = 0x0;

/// Frame types (RFC 9114 section 7.2).
const DATA: u64 = 0x0;
const HEADERS: u64 = 0x1;
const MAX_PUSH_ID: u64 = 0xd;

/// Why a stream cannot be read further.
#[derive(Debug, thiserror::Error)]
pub(super) enum FrameError {
    #[error("a frame of type {kind:#x} declares {length} bytes, and the limit is {limit}")]
    Excessive { kind: u64, length: u64, limit: u64 },
    #[error("the stream ends inside a DATA frame")]
    Truncated,
}

/// Cuts the bytes of a stream where its frames begin and end.
pub(super) struct Frames {
    position: Position,
    /// What h3 cannot use before it is complete.
    held: Vec<u8>,
    /// The longest payload to hold.
    limit: u64,
    /// The HEADERS frames so far: a head, then trailers.
    sections: u64,
}

#[derive(Clone, Copy)]
enum Position {
    /// At the type of a unidirectional stream.
    StreamType,
    /// In a unidirectional stream without frames, of which h3 reads only the beginning.
    Frameless,
    /// At the type and length of a frame.
    Header,
    /// In a DATA frame with this much left, which h3 takes in pieces.
    Data(u64),
    /// In a frame with this much left that h3 takes whole.
    Whole(u64),
    /// In a frame with this much left that means nothing (RFC 9114 section 9).
    Unknown(u64),
}

impl Frames {
    pub(super) fn request(limit: u64) -> Self {
        Self::new(Position::Header, limit)
    }

    pub(super) fn unidirectional() -> Self {
        Self::new(Position::StreamType, CONTROL_FRAME_BYTES)
    }

    fn new(position: Position, limit: u64) -> Self {
        Self {
            position,
            held: Vec::new(),
            limit,
            sections: 0,
        }
    }

    /// Take the next piece for h3 from what has arrived. Without one, nothing is left unread.
    pub(super) fn next(&mut self, unread: &mut Bytes) -> Result<Option<Bytes>, FrameError> {
        while !unread.is_empty() {
            match self.position {
                Position::StreamType => {
                    self.held.push(unread.get_u8());
                    if let Some(kind) = varint(&mut &self.held[..]) {
                        self.position = match kind {
                            CONTROL_STREAM => Position::Header,
                            _ => Position::Frameless,
                        };
                        return Ok(Some(self.release()));
                    }
                }
                Position::Frameless => return Ok(Some(mem::take(unread))),
                Position::Header => {
                    self.held.push(unread.get_u8());
                    let mut header = &self.held[..];
                    if let (Some(kind), Some(length)) = (varint(&mut header), varint(&mut header)) {
                        let piece = self.begin(kind, length)?;
                        if piece.is_some() {
                            return Ok(piece);
                        }
                    }
                }
                Position::Data(left) => {
                    let piece = take(unread, left);
                    self.position = match left - piece.len() as u64 {
                        0 => Position::Header,
                        left => Position::Data(left),
                    };
                    return Ok(Some(piece));
                }
                Position::Whole(left) => {
                    let piece = take(unread, left);
                    if self.held.capacity() - self.held.len() < piece.len() {
                        // Twice as much, but never more than the frame.
                        let more = self.held.len().min(left as usize);
                        self.held.reserve_exact(more.max(piece.len()));
                    }
                    self.held.extend_from_slice(&piece);
                    match left - piece.len() as u64 {
                        0 => {
                            self.position = Position::Header;
                            return Ok(Some(self.release()));
                        }
                        left => self.position = Position::Whole(left),
                    }
                }
                Position::Unknown(left) => match left - take(unread, left).len() as u64 {
                    0 => self.forget(),
                    left => self.position = Position::Unknown(left),
                },
            }
        }
        Ok(None)
    }

    /// Enter the frame whose header is held, and take the piece that this completes.
    fn begin(&mut self, kind: u64, length: u64) -> Result<Option<Bytes>, FrameError> {
        match kind {
            // h3 takes an empty frame for the end of the body
            // (https://github.com/hyperium/h3/issues/372). Outside the body it is an error.
            DATA if length == 0 && self.sections == 1 => {
                self.forget();
                Ok(None)
            }
            DATA => {
                // h3 counts in this type, which has 32 bits on some systems.
                let limit = usize::MAX as u64;
                if length > limit {
                    return Err(FrameError::Excessive {
                        kind,
                        length,
                        limit,
                    });
                }
                if length > 0 {
                    self.position = Position::Data(length);
                }
                Ok(Some(self.release()))
            }
            // What h3 knows, including the types that HTTP/2 left behind (RFC 9114 section 7.2.8).
            HEADERS..=0x9 | MAX_PUSH_ID => {
                let limit = self.limit;
                if length > limit {
                    return Err(FrameError::Excessive {
                        kind,
                        length,
                        limit,
                    });
                }
                if kind == HEADERS {
                    self.sections += 1;
                }
                if length == 0 {
                    return Ok(Some(self.release()));
                }
                self.position = Position::Whole(length);
                Ok(None)
            }
            _ => {
                if length == 0 {
                    self.forget();
                } else {
                    self.position = Position::Unknown(length);
                }
                Ok(None)
            }
        }
    }

    /// What h3 reads when the stream has ended.
    pub(super) fn end(&mut self) -> Result<Option<Bytes>, FrameError> {
        match self.position {
            // h3 would panic (https://github.com/hyperium/h3/issues/386).
            Position::Data(_) => Err(FrameError::Truncated),
            // The beginning of a frame, which h3 reports as incomplete.
            _ => Ok(Some(self.release()).filter(|rest| !rest.is_empty())),
        }
    }

    fn release(&mut self) -> Bytes {
        mem::take(&mut self.held).into()
    }

    /// Leave a frame that h3 does not see.
    fn forget(&mut self) {
        self.held.clear();
        self.position = Position::Header;
    }
}

/// Take at most this much from the front.
fn take(unread: &mut Bytes, most: u64) -> Bytes {
    let most = usize::try_from(most).unwrap_or(usize::MAX);
    unread.split_to(most.min(unread.len()))
}

/// Take a QUIC integer (RFC 9000 section 16) from the front, if it is complete.
fn varint(bytes: &mut &[u8]) -> Option<u64> {
    let first = *bytes.first()?;
    let (integer, rest) = bytes.split_at_checked(1 << (first >> 6))?;
    *bytes = rest;
    let value = integer[1..]
        .iter()
        .fold(u64::from(first & 0x3f), |value, byte| {
            value << 8 | u64::from(*byte)
        });
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: u64 = 100;
    const SETTINGS: u64 = 0x4;
    const GREASE: u64 = 0x21;

    /// Each integer takes four bytes, the length's two high bits included.
    fn header(kind: u64, length: u64) -> Vec<u8> {
        [kind, length]
            .iter()
            .flat_map(|value| (u32::try_from(*value).unwrap() | 0x8000_0000).to_be_bytes())
            .collect()
    }

    fn frame(kind: u64, payload: &[u8]) -> Vec<u8> {
        [header(kind, payload.len() as u64), payload.to_vec()].concat()
    }

    /// What h3 reads from a stream that arrives in chunks of this size and ends.
    fn read(frames: Frames, stream: &[u8], chunk: usize) -> Result<Vec<Bytes>, String> {
        let mut frames = frames;
        let mut pieces = Vec::new();
        for chunk in stream.chunks(chunk) {
            let mut unread = Bytes::copy_from_slice(chunk);
            while let Some(piece) = frames.next(&mut unread).map_err(|e| e.to_string())? {
                pieces.push(piece);
            }
            assert!(unread.is_empty());
        }
        pieces.extend(frames.end().map_err(|error| error.to_string())?);
        Ok(pieces)
    }

    /// Read a request stream whole, and compare with what it is in smaller chunks.
    fn request(stream: &[u8]) -> Result<Vec<Bytes>, String> {
        let whole = read(Frames::request(LIMIT), stream, stream.len());
        for chunk in 1..stream.len() {
            let parts = read(Frames::request(LIMIT), stream, chunk);
            assert_eq!(
                parts.as_ref().map(|pieces| pieces.concat()),
                whole.as_ref().map(|pieces| pieces.concat()),
                "in chunks of {chunk}"
            );
        }
        whole
    }

    #[test]
    fn integers_have_four_sizes() {
        // The examples in RFC 9000 appendix A.1.
        for (bytes, value) in [
            (
                &[0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c][..],
                151_288_809_941_952_652,
            ),
            (&[0x9d, 0x7f, 0x3e, 0x7d], 494_878_333),
            (&[0x7b, 0xbd], 15_293),
            (&[0x25], 37),
            (&[0x40, 0x25], 37),
        ] {
            assert_eq!(varint(&mut &bytes[..]), Some(value));
            assert_eq!(varint(&mut &bytes[..bytes.len() - 1]), None);
        }
    }

    #[test]
    fn headers_may_encode_their_integers_in_any_size() {
        let encode = |value: u8, size: u32| {
            let mut bytes = vec![0; 1 << size];
            bytes[0] = (size as u8) << 6;
            *bytes.last_mut().unwrap() |= value;
            bytes
        };
        for (kind, length) in (0..4).flat_map(|kind| (0..4).map(move |length| (kind, length))) {
            let header = [
                encode(DATA as u8, kind),
                // A length that fits the six bits of the smallest size.
                encode(3, length),
            ]
            .concat();
            let head = [
                encode(HEADERS as u8, kind),
                encode(4, length),
                b"head".to_vec(),
            ]
            .concat();
            let stream = [&head[..], &header, b"abc", &header, b"def"].concat();
            let expected = [&head[..], &header, b"abc", &header, b"def"];
            assert_eq!(request(&stream).unwrap(), expected, "{kind} {length}");
        }
    }

    #[test]
    fn a_frame_takes_memory_as_it_arrives() {
        const LENGTH: usize = 100_000;

        let stream = frame(HEADERS, &[0; LENGTH]);
        let mut frames = Frames::request(LENGTH as u64);
        let mut arrived = 0;
        for chunk in stream.chunks(1_000) {
            let piece = frames.next(&mut Bytes::copy_from_slice(chunk)).unwrap();
            arrived += chunk.len();
            if arrived < stream.len() {
                assert_eq!(piece, None);
                let memory = frames.held.capacity();
                assert!(memory <= stream.len(), "{memory} of {}", stream.len());
                assert!(memory <= 2 * arrived, "{memory} after {arrived}");
            } else {
                assert_eq!(piece.unwrap(), stream);
            }
        }
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn data_frames_may_be_as_long_as_integers_are() {
        let header = [
            &[0x00][..],
            &(0xc000_0000_0000_0000_u64 | (1 << 40)).to_be_bytes(),
        ]
        .concat();
        let stream = [frame(HEADERS, b"head"), header.clone(), b"abc".to_vec()].concat();
        let mut frames = Frames::request(LIMIT);
        let mut unread = Bytes::from(stream);
        let mut pieces = Vec::new();
        while let Some(piece) = frames.next(&mut unread).unwrap() {
            pieces.push(piece);
        }
        assert_eq!(pieces, [&frame(HEADERS, b"head")[..], &header, b"abc"]);
    }

    #[test]
    fn every_read_ends_with_a_frame_or_a_piece_of_data() {
        let stream = [
            frame(HEADERS, b"head"),
            frame(DATA, b"a"),
            frame(DATA, b"bc"),
            frame(HEADERS, b"trailers"),
        ]
        .concat();
        let expected = [
            frame(HEADERS, b"head"),
            header(DATA, 1),
            b"a".to_vec(),
            header(DATA, 2),
            b"bc".to_vec(),
            frame(HEADERS, b"trailers"),
        ];
        assert_eq!(request(&stream).unwrap(), expected);
    }

    #[test]
    fn data_passes_through_as_it_arrives() {
        let stream = [frame(HEADERS, b"head"), header(DATA, 1 << 29)].concat();
        let mut frames = Frames::request(LIMIT);
        let mut unread = Bytes::from(stream);
        while frames.next(&mut unread).unwrap().is_some() {}
        for _ in 0..3 {
            let arrived = Bytes::from(vec![b'x'; 1 << 20]);
            assert_eq!(frames.next(&mut arrived.clone()).unwrap(), Some(arrived));
        }
    }

    #[test]
    fn unknown_frames_are_dropped() {
        let large = vec![0; 1_000];
        let stream = [
            frame(GREASE, &large),
            frame(HEADERS, b"head"),
            frame(GREASE, b""),
            frame(DATA, b"a"),
            // The stream type of WebTransport, which has no length there.
            frame(0x41, b"session"),
            // Reserved in HTTP/2, but not in HTTP/3.
            frame(0xa, b""),
            frame(0xe, &large),
        ]
        .concat();
        let expected = [frame(HEADERS, b"head"), header(DATA, 1), b"a".to_vec()];
        assert_eq!(request(&stream).unwrap(), expected);
    }

    #[test]
    fn a_frame_is_refused_when_it_declares_more_than_the_limit() {
        for kind in (HEADERS..=0x9).chain([MAX_PUSH_ID]) {
            let payload = vec![0; LIMIT as usize];
            let expected = [frame(kind, &payload)];
            assert_eq!(request(&expected[0]).unwrap(), expected, "{kind:#x}");
            // Nothing of it has arrived.
            assert!(request(&header(kind, LIMIT + 1)).is_err(), "{kind:#x}");
        }
        let trailers = [frame(HEADERS, b"head"), header(HEADERS, LIMIT + 1)].concat();
        assert!(request(&trailers).is_err());
    }

    #[test]
    fn empty_data_frames_disappear_from_bodies_only() {
        let empty = frame(DATA, b"");
        let head = frame(HEADERS, b"head");
        let body = [&head[..], &empty, &frame(DATA, b"a"), &empty].concat();
        let expected = [head.clone(), header(DATA, 1), b"a".to_vec()];
        assert_eq!(request(&body).unwrap(), expected);

        let before_the_head = [&empty[..], &head].concat();
        assert_eq!(request(&before_the_head).unwrap().concat(), before_the_head);
        let after_trailers = [&head[..], &head, &empty].concat();
        assert_eq!(request(&after_trailers).unwrap().concat(), after_trailers);
    }

    #[test]
    fn incomplete_streams_do_not_look_complete() {
        let data = [frame(HEADERS, b"head"), header(DATA, 2), b"a".to_vec()].concat();
        assert!(request(&data).is_err());
        assert!(request(&data[..data.len() - 1]).is_err());
        // h3 reports these from the part that it gets.
        let head = frame(HEADERS, b"head");
        for length in 1..head.len() {
            assert_eq!(request(&head[..length]).unwrap(), [&head[..length]]);
        }
        let unknown = frame(GREASE, b"grease");
        for length in 1..unknown.len() {
            assert_eq!(
                request(&unknown[..length]).unwrap(),
                [&unknown[..length.min(8)]]
            );
        }
    }

    #[test]
    fn only_control_streams_have_frames() {
        let settings = frame(SETTINGS, b"settings");
        let control = [&[0x40, 0x00][..], &settings, &frame(GREASE, b"grease")].concat();
        for chunk in 1..=control.len() {
            let pieces = read(Frames::unidirectional(), &control, chunk).unwrap();
            assert_eq!(pieces, [&[0x40, 0x00][..], &settings]);
        }
        let excessive = [&[0x00][..], &header(SETTINGS, CONTROL_FRAME_BYTES + 1)].concat();
        assert!(read(Frames::unidirectional(), &excessive, 64).is_err());

        // A QPACK encoder stream, with what would be a refused frame on a control stream.
        let other = [&[0x02][..], &header(SETTINGS, CONTROL_FRAME_BYTES + 1)].concat();
        for chunk in 1..=other.len() {
            let pieces = read(Frames::unidirectional(), &other, chunk).unwrap();
            assert_eq!(pieces.concat(), other);
        }
    }
}
