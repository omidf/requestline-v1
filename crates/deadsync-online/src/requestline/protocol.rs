//! Websocket messages, protocol 1. Every message both ways is one JSON text
//! frame with a `type`. Unknown types and fields are ignored.

use serde::{Deserialize, Serialize};

pub(super) const PROTOCOL: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(super) enum Outgoing {
    Hello {
        protocol: u32,
    },
    Ping,
    LibraryUpload,
    #[serde(rename_all = "camelCase")]
    LibraryUploaded {
        upload_id: String,
        hash: String,
    },
    #[serde(rename_all = "camelCase")]
    Started {
        song_id: String,
        chart_hash: String,
    },
    /// The in-game requests switch. Closing counts as the streamer closing by
    /// hand: the page stays closed until opened again. Answered with a state.
    SetOpen {
        open: bool,
    },
    /// The song finished, failed or was quit (not restarted). No reply.
    #[serde(rename_all = "camelCase")]
    Ended {
        song_id: String,
    },
}

impl Outgoing {
    pub(super) fn to_text(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(super) enum Incoming {
    #[serde(rename_all = "camelCase")]
    Hello {
        #[serde(default)]
        page_name: Option<String>,
        #[serde(default)]
        library_hash: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    State {
        #[serde(default)]
        open: bool,
        #[serde(default)]
        closed_by: Option<String>,
        /// The whole queue, first to play first. An entry missing its id or
        /// song id is dropped; the rest of the message still counts.
        #[serde(default, deserialize_with = "lenient_queue")]
        queue: Vec<QueueEntry>,
    },
    Pong,
    #[serde(rename_all = "camelCase")]
    LibraryUpload {
        upload_id: String,
        url: String,
        #[serde(default)]
        max_bytes: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    LibraryUpdated {
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        song_count: u64,
        #[serde(default)]
        pack_count: u64,
        #[serde(default)]
        skipped: u64,
    },
    #[serde(rename_all = "camelCase")]
    Started {
        #[serde(default)]
        song_id: Option<String>,
        #[serde(default)]
        removed: Option<Removed>,
        #[serde(default)]
        repeat: bool,
    },
    #[serde(rename_all = "camelCase")]
    Error {
        #[serde(default)]
        code: String,
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        reason: Option<String>,
        /// Which message a `refused` error is about.
        #[serde(default, rename = "for")]
        for_type: Option<String>,
        #[serde(default)]
        upload_id: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

/// One request in the page's queue: what the game needs to find and credit it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueEntry {
    pub id: String,
    /// `Pack/SongDir`, the same id the library upload uses.
    pub song_id: String,
    #[serde(default)]
    pub title: String,
    /// The requester's ArrowCloud alias.
    #[serde(default)]
    pub name: String,
    /// The chart asked for; `None` leaves the choice to the player. A chart
    /// that doesn't parse counts as none rather than losing the request.
    #[serde(default, deserialize_with = "lenient")]
    pub chart: Option<RequestedChart>,
}

/// The chart a viewer picked.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RequestedChart {
    pub hash: String,
    #[serde(default, deserialize_with = "lenient")]
    pub difficulty: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub meter: Option<u32>,
}

/// `None` for a value that is missing or doesn't parse as `T`.
fn lenient<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(de)?;
    Ok(serde_json::from_value(value).ok())
}

/// The queue's entries that parse; a malformed one is skipped, not fatal.
fn lenient_queue<'de, D>(de: D) -> Result<Vec<QueueEntry>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(de)?;
    let serde_json::Value::Array(items) = value else {
        return Ok(Vec::new());
    };
    Ok(items
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect())
}

/// The request that left the queue. Only what the log line needs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(super) struct Removed {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

impl Incoming {
    /// `None` for text that isn't a message object at all.
    pub(super) fn parse(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_queue_keeps_order_and_skips_what_it_cannot_read() {
        let Some(Incoming::State { queue, .. }) = Incoming::parse(
            r#"{"type":"state","open":true,"queue":[
                {"id":"r1","songId":"p/a","title":"A","pack":"p","seconds":90,"name":"one","requestedAt":"t"},
                {"songId":"p/no-id","title":"No id","name":"two"},
                {"id":"r3","songId":"p/c","title":"C","name":"three","chart":{"hash":"cd","meter":"high"}},
                {"id":"r4","songId":"p/d","title":"D","name":"four","chart":{"difficulty":"hard"}},
                {"id":"r5","songId":"p/e","name":"five","chart":{"hash":"ef","difficulty":"easy","meter":3}}
            ]}"#,
        ) else {
            panic!("not a state message");
        };
        let ids: Vec<&str> = queue.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["r1", "r3", "r4", "r5"], "the entry without an id is skipped");
        assert_eq!(queue[0].chart, None, "no chart: the player's choice");
        assert_eq!(
            queue[1].chart,
            Some(RequestedChart {
                hash: "cd".into(),
                difficulty: None,
                meter: None,
            }),
            "an unreadable meter loses the meter, not the chart"
        );
        assert_eq!(queue[2].chart, None, "a chart without a hash is no chart");
        assert_eq!(queue[3].title, "", "a missing title is empty, not fatal");
        assert_eq!(queue[3].chart.as_ref().unwrap().meter, Some(3));

        let Some(Incoming::State { queue, .. }) =
            Incoming::parse(r#"{"type":"state","open":true,"queue":null}"#)
        else {
            panic!("not a state message");
        };
        assert!(queue.is_empty());
    }

    #[test]
    fn outgoing_messages_match_the_contract() {
        let cases = [
            (
                Outgoing::Hello { protocol: 1 },
                r#"{"type":"hello","protocol":1}"#,
            ),
            (Outgoing::Ping, r#"{"type":"ping"}"#),
            (
                Outgoing::SetOpen { open: true },
                r#"{"type":"set-open","open":true}"#,
            ),
            (Outgoing::LibraryUpload, r#"{"type":"library-upload"}"#),
            (
                Outgoing::LibraryUploaded {
                    upload_id: "u1".into(),
                    hash: "sha256:ab".into(),
                },
                r#"{"type":"library-uploaded","uploadId":"u1","hash":"sha256:ab"}"#,
            ),
            (
                Outgoing::Started {
                    song_id: "altruism2/Bounce Back".into(),
                    chart_hash: "5d8c5de5bce738f4".into(),
                },
                r#"{"type":"started","songId":"altruism2/Bounce Back","chartHash":"5d8c5de5bce738f4"}"#,
            ),
            (
                Outgoing::Ended {
                    song_id: "altruism2/Bounce Back".into(),
                },
                r#"{"type":"ended","songId":"altruism2/Bounce Back"}"#,
            ),
        ];
        for (message, json) in cases {
            assert_eq!(message.to_text(), json);
        }
    }

    #[test]
    fn appendix_a_server_messages_parse() {
        assert_eq!(
            Incoming::parse(
                r#"{"type":"hello","protocol":1,"role":"game","userId":"27cf","pageName":"Wafles","isOwner":true,"libraryHash":null}"#
            ),
            Some(Incoming::Hello {
                page_name: Some("Wafles".into()),
                library_hash: None,
            })
        );
        assert_eq!(
            Incoming::parse(
                r#"{"type":"state","open":true,"closedBy":null,"gameConnected":true,"queue":[{"id":"r1","songId":"altruism2/Bounce Back","title":"Bounce Back","pack":"altruism2","seconds":131,"name":"AutoPlayCPU","requestedAt":"x","chart":{"hash":"ab","difficulty":"challenge","meter":12}}],"nowPlaying":null,"rules":{"perPerson":2,"cooldownSeconds":60,"maxQueue":20,"meterMin":null,"meterMax":null,"packs":null}}"#
            ),
            Some(Incoming::State {
                open: true,
                closed_by: None,
                queue: vec![QueueEntry {
                    id: "r1".into(),
                    song_id: "altruism2/Bounce Back".into(),
                    title: "Bounce Back".into(),
                    name: "AutoPlayCPU".into(),
                    chart: Some(RequestedChart {
                        hash: "ab".into(),
                        difficulty: Some("challenge".into()),
                        meter: Some(12),
                    }),
                }],
            })
        );
        assert_eq!(
            Incoming::parse(
                r#"{"type":"state","open":false,"closedBy":"idle","gameConnected":true,"queue":[],"nowPlaying":{"songId":"a/b"},"rules":{}}"#
            ),
            Some(Incoming::State {
                open: false,
                closed_by: Some("idle".into()),
                queue: Vec::new(),
            })
        );
        assert_eq!(Incoming::parse(r#"{"type":"pong"}"#), Some(Incoming::Pong));
        assert_eq!(
            Incoming::parse(
                r#"{"type":"library-upload","uploadId":"50fc","url":"https://s3/x?X-Amz=1","expiresIn":300,"maxBytes":10485760}"#
            ),
            Some(Incoming::LibraryUpload {
                upload_id: "50fc".into(),
                url: "https://s3/x?X-Amz=1".into(),
                max_bytes: Some(10_485_760),
            })
        );
        assert_eq!(
            Incoming::parse(
                r#"{"type":"library-updated","version":"50fc","songCount":144,"packCount":5,"chartCount":721,"updatedAt":"t","skipped":0}"#
            ),
            Some(Incoming::LibraryUpdated {
                version: Some("50fc".into()),
                song_count: 144,
                pack_count: 5,
                skipped: 0,
            })
        );
        assert_eq!(
            Incoming::parse(
                r#"{"type":"started","songId":"altruism2/Bounce Back","removed":{"id":"r1","songId":"altruism2/Bounce Back","title":"Bounce Back","pack":"altruism2","name":"AutoPlayCPU","requestedAt":"t"},"repeat":false}"#
            ),
            Some(Incoming::Started {
                song_id: Some("altruism2/Bounce Back".into()),
                removed: Some(Removed {
                    title: Some("Bounce Back".into()),
                    name: Some("AutoPlayCPU".into()),
                }),
                repeat: false,
            })
        );
        assert_eq!(
            Incoming::parse(r#"{"type":"started","songId":"a/b","removed":null,"repeat":true}"#),
            Some(Incoming::Started {
                song_id: Some("a/b".into()),
                removed: None,
                repeat: true,
            })
        );
        assert_eq!(
            Incoming::parse(
                r#"{"type":"error","code":"library-rejected","message":"bad gzip","uploadId":"u1"}"#
            ),
            Some(Incoming::Error {
                code: "library-rejected".into(),
                message: Some("bad gzip".into()),
                reason: None,
                for_type: None,
                upload_id: Some("u1".into()),
            })
        );
        assert!(matches!(
            Incoming::parse(
                r#"{"type":"error","code":"unsupported-protocol","message":"m","supported":[1]}"#
            ),
            Some(Incoming::Error { code, .. }) if code == "unsupported-protocol"
        ));
        assert!(matches!(
            Incoming::parse(r#"{"type":"error","code":"refused","reason":"invalid","for":"started"}"#),
            Some(Incoming::Error { reason: Some(r), .. }) if r == "invalid"
        ));
        assert!(matches!(
            Incoming::parse(r#"{"type":"error","code":"refused","reason":"invalid","for":"ended"}"#),
            Some(Incoming::Error { for_type: Some(f), .. }) if f == "ended"
        ));
    }

    #[test]
    fn unknown_types_parse_as_unknown() {
        assert_eq!(
            Incoming::parse(r#"{"type":"owner-state","x":1}"#),
            Some(Incoming::Unknown)
        );
        assert_eq!(
            Incoming::parse(r#"{"type":"my-requests","requests":[]}"#),
            Some(Incoming::Unknown)
        );
        assert_eq!(Incoming::parse("not json"), None);
    }
}
