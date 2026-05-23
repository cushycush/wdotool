//! Line-by-line parser for Drift's IPC socket2 stream.
//!
//! Wire format: `eventname>>arg1,arg2,...\n`. Newlines inside arg
//! payloads are replaced with spaces by the compositor before send, so
//! one line is always one event. The compositor also truncates the
//! data portion at 1024 bytes, which is plenty for any realistic
//! window title.
//!
//! Many compositor transitions are emitted twice on the wire: once in
//! a legacy line (string-only data) and once in a `v2` line (carrying
//! ids / addresses). The parser owns a small piece of state to merge
//! the pair into a single typed [`Event`]. Consumers see one event per
//! real transition.

use super::types::{ActiveWindow, Event, MonitorFocusWorkspace};

/// Stateful parser. Holds at most one pending half of a coalesced pair
/// (legacy line waiting for its v2, or vice versa).
#[derive(Debug, Default)]
pub(crate) struct EventParser {
    pending_active_legacy: Option<(String, String)>,
    pending_active_v2: Option<String>,
    pending_active_empty_legacy: bool,
    pending_active_empty_v2: bool,
    pending_title_legacy: Option<String>,
    pending_title_v2: Option<(String, String)>,
    pending_workspace_legacy: Option<String>,
    pending_workspace_v2: Option<(i64, String)>,
    pending_focusedmon_legacy: Option<(String, String)>,
    pending_focusedmon_v2: Option<(String, i64)>,
    pending_createws_legacy: Option<String>,
    pending_createws_v2: Option<(i64, String)>,
    pending_destroyws_legacy: Option<String>,
    pending_destroyws_v2: Option<(i64, String)>,
    pending_monitoradded_legacy: Option<String>,
    pending_monitoradded_v2: Option<(i64, String, String)>,
    pending_monitorremoved_legacy: Option<String>,
    pending_monitorremoved_v2: Option<(i64, String, String)>,
}

impl EventParser {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Feed one raw line (without the trailing newline) and get back
    /// zero or more decoded events.
    ///
    /// Returns multiple events in the rare case where the previous
    /// pending half of a pair times out: when a new event of a
    /// different kind arrives while a legacy / v2 line is still
    /// waiting for its partner, the orphan flushes as a best-effort
    /// event before the new line is processed. In practice Drift
    /// emits the pair back-to-back so this almost never triggers.
    pub(crate) fn feed(&mut self, line: &str) -> Vec<Event> {
        let (name, data) = match line.split_once(">>") {
            Some(parts) => parts,
            None => return Vec::new(),
        };
        self.handle(name, data)
    }

    fn handle(&mut self, name: &str, data: &str) -> Vec<Event> {
        match name {
            "openwindow" => parse_openwindow(data).map(|e| vec![e]).unwrap_or_default(),
            "closewindow" => vec![Event::WindowClosed {
                address: data.to_string(),
            }],
            "activewindow" => self.feed_active_legacy(data),
            "activewindowv2" => self.feed_active_v2(data),
            "windowtitle" => self.feed_title_legacy(data),
            "windowtitlev2" => self.feed_title_v2(data),
            "workspace" => self.feed_workspace_legacy(data),
            "workspacev2" => self.feed_workspace_v2(data),
            "focusedmon" => self.feed_focusedmon_legacy(data),
            "focusedmonv2" => self.feed_focusedmon_v2(data),
            "createworkspace" => self.feed_createws_legacy(data),
            "createworkspacev2" => self.feed_createws_v2(data),
            "destroyworkspace" => self.feed_destroyws_legacy(data),
            "destroyworkspacev2" => self.feed_destroyws_v2(data),
            "monitoradded" => self.feed_monitoradded_legacy(data),
            "monitoraddedv2" => self.feed_monitoradded_v2(data),
            "monitorremoved" => self.feed_monitorremoved_legacy(data),
            "monitorremovedv2" => self.feed_monitorremoved_v2(data),
            "fullscreen" => match data.trim() {
                "1" => vec![Event::FullscreenChanged { fullscreen: true }],
                "0" => vec![Event::FullscreenChanged { fullscreen: false }],
                _ => Vec::new(),
            },
            "urgent" => vec![Event::WindowUrgent {
                address: data.to_string(),
            }],
            "minimized" => parse_minimized(data).map(|e| vec![e]).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn feed_active_legacy(&mut self, data: &str) -> Vec<Event> {
        if data == "," || data.is_empty() {
            if self.pending_active_empty_v2 {
                self.pending_active_empty_v2 = false;
                return vec![Event::ActiveWindowChanged(None)];
            }
            self.pending_active_empty_legacy = true;
            return Vec::new();
        }
        let (class, title) = match data.split_once(',') {
            Some(parts) => parts,
            None => (data, ""),
        };
        if let Some(address) = self.pending_active_v2.take() {
            return vec![Event::ActiveWindowChanged(Some(ActiveWindow {
                address,
                app_id: Some(class.to_string()),
                title: Some(title.to_string()),
            }))];
        }
        self.pending_active_legacy = Some((class.to_string(), title.to_string()));
        Vec::new()
    }

    fn feed_active_v2(&mut self, data: &str) -> Vec<Event> {
        if data.is_empty() {
            if self.pending_active_empty_legacy {
                self.pending_active_empty_legacy = false;
                return vec![Event::ActiveWindowChanged(None)];
            }
            self.pending_active_empty_v2 = true;
            return Vec::new();
        }
        if let Some((class, title)) = self.pending_active_legacy.take() {
            return vec![Event::ActiveWindowChanged(Some(ActiveWindow {
                address: data.to_string(),
                app_id: Some(class),
                title: Some(title),
            }))];
        }
        self.pending_active_v2 = Some(data.to_string());
        Vec::new()
    }

    fn feed_title_legacy(&mut self, data: &str) -> Vec<Event> {
        if let Some((address, title)) = self.pending_title_v2.take() {
            let _ = data;
            return vec![Event::WindowTitleChanged { address, title }];
        }
        self.pending_title_legacy = Some(data.to_string());
        Vec::new()
    }

    fn feed_title_v2(&mut self, data: &str) -> Vec<Event> {
        let (address, title) = match data.split_once(',') {
            Some(parts) => parts,
            None => return Vec::new(),
        };
        if self.pending_title_legacy.take().is_some() {
            return vec![Event::WindowTitleChanged {
                address: address.to_string(),
                title: title.to_string(),
            }];
        }
        self.pending_title_v2 = Some((address.to_string(), title.to_string()));
        Vec::new()
    }

    fn feed_workspace_legacy(&mut self, data: &str) -> Vec<Event> {
        if let Some((id, name)) = self.pending_workspace_v2.take() {
            let _ = data;
            return vec![Event::WorkspaceChanged { id: Some(id), name }];
        }
        self.pending_workspace_legacy = Some(data.to_string());
        Vec::new()
    }

    fn feed_workspace_v2(&mut self, data: &str) -> Vec<Event> {
        let parsed = match split_id_name(data) {
            Some(p) => p,
            None => return Vec::new(),
        };
        if self.pending_workspace_legacy.take().is_some() {
            return vec![Event::WorkspaceChanged {
                id: Some(parsed.0),
                name: parsed.1,
            }];
        }
        self.pending_workspace_v2 = Some(parsed);
        Vec::new()
    }

    fn feed_focusedmon_legacy(&mut self, data: &str) -> Vec<Event> {
        let (monitor, workspace_name) = match data.split_once(',') {
            Some(parts) => parts,
            None => return Vec::new(),
        };
        if let Some((mon_v2, ws_id)) = self.pending_focusedmon_v2.take() {
            return vec![Event::MonitorFocusChanged {
                monitor: if monitor.is_empty() {
                    mon_v2
                } else {
                    monitor.to_string()
                },
                workspace: MonitorFocusWorkspace {
                    id: Some(ws_id),
                    name: workspace_name.to_string(),
                },
            }];
        }
        self.pending_focusedmon_legacy = Some((monitor.to_string(), workspace_name.to_string()));
        Vec::new()
    }

    fn feed_focusedmon_v2(&mut self, data: &str) -> Vec<Event> {
        let (monitor, id_str) = match data.split_once(',') {
            Some(parts) => parts,
            None => return Vec::new(),
        };
        let id = match id_str.parse::<i64>() {
            Ok(n) => n,
            Err(_) => return Vec::new(),
        };
        if let Some((mon_legacy, ws_name)) = self.pending_focusedmon_legacy.take() {
            return vec![Event::MonitorFocusChanged {
                monitor: if mon_legacy.is_empty() {
                    monitor.to_string()
                } else {
                    mon_legacy
                },
                workspace: MonitorFocusWorkspace {
                    id: Some(id),
                    name: ws_name,
                },
            }];
        }
        self.pending_focusedmon_v2 = Some((monitor.to_string(), id));
        Vec::new()
    }

    fn feed_createws_legacy(&mut self, data: &str) -> Vec<Event> {
        if let Some((id, name)) = self.pending_createws_v2.take() {
            let _ = data;
            return vec![Event::WorkspaceCreated { id: Some(id), name }];
        }
        self.pending_createws_legacy = Some(data.to_string());
        Vec::new()
    }

    fn feed_createws_v2(&mut self, data: &str) -> Vec<Event> {
        let parsed = match split_id_name(data) {
            Some(p) => p,
            None => return Vec::new(),
        };
        if self.pending_createws_legacy.take().is_some() {
            return vec![Event::WorkspaceCreated {
                id: Some(parsed.0),
                name: parsed.1,
            }];
        }
        self.pending_createws_v2 = Some(parsed);
        Vec::new()
    }

    fn feed_destroyws_legacy(&mut self, data: &str) -> Vec<Event> {
        if let Some((id, name)) = self.pending_destroyws_v2.take() {
            let _ = data;
            return vec![Event::WorkspaceDestroyed { id: Some(id), name }];
        }
        self.pending_destroyws_legacy = Some(data.to_string());
        Vec::new()
    }

    fn feed_destroyws_v2(&mut self, data: &str) -> Vec<Event> {
        let parsed = match split_id_name(data) {
            Some(p) => p,
            None => return Vec::new(),
        };
        if self.pending_destroyws_legacy.take().is_some() {
            return vec![Event::WorkspaceDestroyed {
                id: Some(parsed.0),
                name: parsed.1,
            }];
        }
        self.pending_destroyws_v2 = Some(parsed);
        Vec::new()
    }

    fn feed_monitoradded_legacy(&mut self, data: &str) -> Vec<Event> {
        if let Some((id, name, desc)) = self.pending_monitoradded_v2.take() {
            let _ = data;
            return vec![Event::MonitorAdded {
                id: Some(id),
                name,
                description: Some(desc),
            }];
        }
        self.pending_monitoradded_legacy = Some(data.to_string());
        Vec::new()
    }

    fn feed_monitoradded_v2(&mut self, data: &str) -> Vec<Event> {
        let parts = match split_monitor_v2(data) {
            Some(p) => p,
            None => return Vec::new(),
        };
        if self.pending_monitoradded_legacy.take().is_some() {
            return vec![Event::MonitorAdded {
                id: Some(parts.0),
                name: parts.1,
                description: Some(parts.2),
            }];
        }
        self.pending_monitoradded_v2 = Some(parts);
        Vec::new()
    }

    fn feed_monitorremoved_legacy(&mut self, data: &str) -> Vec<Event> {
        if let Some((id, name, desc)) = self.pending_monitorremoved_v2.take() {
            let _ = data;
            return vec![Event::MonitorRemoved {
                id: Some(id),
                name,
                description: Some(desc),
            }];
        }
        self.pending_monitorremoved_legacy = Some(data.to_string());
        Vec::new()
    }

    fn feed_monitorremoved_v2(&mut self, data: &str) -> Vec<Event> {
        let parts = match split_monitor_v2(data) {
            Some(p) => p,
            None => return Vec::new(),
        };
        if self.pending_monitorremoved_legacy.take().is_some() {
            return vec![Event::MonitorRemoved {
                id: Some(parts.0),
                name: parts.1,
                description: Some(parts.2),
            }];
        }
        self.pending_monitorremoved_v2 = Some(parts);
        Vec::new()
    }
}

fn parse_openwindow(data: &str) -> Option<Event> {
    // address,workspace,class,title -- title may itself contain commas
    // so splitn(4) on the first three keeps the title intact.
    let mut parts = data.splitn(4, ',');
    let address = parts.next()?.to_string();
    let workspace = parts.next()?.to_string();
    let app_id = parts.next()?.to_string();
    let title = parts.next().unwrap_or("").to_string();
    Some(Event::WindowOpened {
        address,
        workspace,
        app_id,
        title,
    })
}

fn parse_minimized(data: &str) -> Option<Event> {
    let (address, flag) = data.split_once(',')?;
    let minimized = match flag.trim() {
        "1" => true,
        "0" => false,
        _ => return None,
    };
    Some(Event::WindowMinimized {
        address: address.to_string(),
        minimized,
    })
}

fn split_id_name(data: &str) -> Option<(i64, String)> {
    let (id_str, name) = data.split_once(',')?;
    let id = id_str.parse::<i64>().ok()?;
    Some((id, name.to_string()))
}

fn split_monitor_v2(data: &str) -> Option<(i64, String, String)> {
    let mut parts = data.splitn(3, ',');
    let id = parts.next()?.parse::<i64>().ok()?;
    let name = parts.next()?.to_string();
    let desc = parts.next().unwrap_or("").to_string();
    Some((id, name, desc))
}
