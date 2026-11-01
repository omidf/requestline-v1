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
    fn outgoing_messages_match_the_contract() {
        let cases = [
            (
                Outgoing::Hello { protocol: 1 },
                r#"{"type":"hello","protocol":1}"#,
            ),
            (Outgoing::Ping, r#"{"type":"ping"}"#),
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
            })
        );
        assert_eq!(
            Incoming::parse(
                r#"{"type":"state","open":false,"closedBy":"idle","gameConnected":true,"queue":[],"nowPlaying":{"songId":"a/b"},"rules":{}}"#
            ),
            Some(Incoming::State {
                open: false,
                closed_by: Some("idle".into()),
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
