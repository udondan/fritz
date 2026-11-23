//! Restarting a device: `DeviceConfig:Reboot` as a critical change with a
//! plan, a token and the audit log like any other, and
//! [waiting](Changer::wait) until the device is back.
//!
//! The [plan](Changer::plan_reboot) names the device. The
//! [restart](Changer::reboot) is sent once and never again on its own; it
//! is [accepted](State::Accepted) if the device answers the call, and
//! [unknown](State::Unknown) if the connection breaks before the answer.
//! Waiting only reads `DeviceInfo:GetInfo`: the device is
//! [back](State::Back) once it answers with a lower uptime than before the
//! restart, and the same serial number. Another serial number is
//! [`Error::OtherDevice`].

use std::fmt;
use std::time::Duration;

use serde::Serialize;
use tokio::time::Instant;
use tr064::{Action, Client, Service};

use super::{Changer, Error, Event, subject};
use crate::generic;
use crate::identity::Identity;
use crate::redact;

const SERVICE: &str = "DeviceConfig1";
const ACTION: &str = "Reboot";

/// What a restart takes down.
pub const WARNING: &str = "the device restarts: internet, WLAN and telephony are down \
for a few minutes, and so is everything connected through it";

/// A planned restart.
#[derive(Debug, Clone, Serialize)]
pub struct RebootPlan {
    pub device: Identity,
    /// The model, like `FRITZ!Repeater 2400`.
    pub model: Option<String>,
    /// The name of the device.
    pub name: Option<String>,
    /// Seconds since the last start.
    pub uptime: Option<u64>,
    /// The class of `Reboot`, like `critical: changes DeviceConfig`.
    pub class: String,
    /// See [`WARNING`].
    pub warning: &'static str,
    /// The token to [restart](Changer::reboot) the device with.
    pub token: String,
    /// Seconds the token is valid.
    pub expires_in: u64,
}

/// Where a restart stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// The device accepted the restart.
    Accepted,
    /// The device stopped answering after it accepted the restart.
    Rebooting,
    /// The device answers again, and its uptime is lower than before.
    Back,
    /// The device is not back within the time to wait.
    Timeout,
    /// The connection broke while the restart was sent, so whether the
    /// device got it is unknown.
    Unknown,
}

impl State {
    /// What the state means, for people.
    pub fn meaning(self) -> &'static str {
        match self {
            Self::Accepted => "the device accepted the restart",
            Self::Rebooting => "the device stopped answering",
            Self::Back => "the device restarted and answers again",
            Self::Timeout => "the device is not back within the time to wait",
            Self::Unknown => {
                "the connection broke while the restart was sent; the device may or may not restart"
            }
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Accepted => "accepted",
            Self::Rebooting => "rebooting",
            Self::Back => "back",
            Self::Timeout => "timeout",
            Self::Unknown => "unknown",
        };
        f.write_str(name)
    }
}

/// The outcome of a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub device: Identity,
    pub outcome: State,
    /// The uptime before the restart, in seconds.
    pub uptime_before: Option<u64>,
    /// The uptime the device answers once it is back, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime: Option<u64>,
    /// Seconds waited for the device.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waited: Option<u64>,
    /// Why the outcome is [unknown](State::Unknown).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// How to wait for a device to come back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wait {
    /// The time between two reads.
    pub interval: Duration,
    /// The longest time to wait.
    pub budget: Duration,
}

impl Default for Wait {
    /// Every 5 seconds, for at most 5 minutes.
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(5),
            budget: Duration::from_mins(5),
        }
    }
}

/// A restart that was sent.
#[derive(Debug)]
pub struct Reboot {
    report: Report,
    udn: String,
    probe: Probe,
}

impl Reboot {
    /// The outcome of sending it: [accepted](State::Accepted) or
    /// [unknown](State::Unknown).
    pub fn report(&self) -> &Report {
        &self.report
    }
}

/// `DeviceInfo:GetInfo`, the only action read to wait for a device.
#[derive(Debug)]
pub(super) struct Probe {
    service: Service,
    action: Action,
}

/// What `DeviceInfo:GetInfo` answers.
#[derive(Debug)]
pub(super) struct Info {
    model: Option<String>,
    pub(super) serial: Option<String>,
    uptime: Option<u64>,
}

impl Probe {
    pub(super) async fn new(client: &Client) -> Result<Self, Error> {
        let (service, scpd) = generic::service(client, "DeviceInfo1").await?;
        let action = generic::action(&service, &scpd, "GetInfo")?.clone();
        Ok(Self { service, action })
    }

    /// A new probe and what it reads first.
    async fn first(client: &Client) -> Result<(Self, Info), Error> {
        let probe = Self::new(client).await?;
        let info = probe.read(client).await?;
        Ok((probe, info))
    }

    pub(super) async fn read(&self, client: &Client) -> Result<Info, Error> {
        let out = generic::send(client, &self.service, &self.action, &[]).await?;
        let get = |name: &str| {
            out.iter()
                .find(|(n, _)| n == name)
                .map(|(_, value)| value.clone())
        };
        Ok(Info {
            model: get("NewModelName").map(|v| redact::value("NewModelName", &v)),
            serial: get("NewSerialNumber"),
            uptime: get("NewUpTime").and_then(|v| v.parse().ok()),
        })
    }
}

impl Changer {
    /// Plans a restart: checks it like any [plan](Self::plan), names the
    /// device and issues a token.
    ///
    /// # Errors
    ///
    /// The errors of [`plan`](Self::plan) and of reading the device.
    pub async fn plan_reboot(&mut self) -> Result<RebootPlan, Error> {
        let plan = self.plan(SERVICE, ACTION, &[]).await?;
        let description = self.client.description().await?;
        let info = Probe::new(&self.client).await?.read(&self.client).await?;
        Ok(RebootPlan {
            device: plan.device,
            model: info.model,
            name: description
                .device
                .friendly_name
                .map(|v| redact::value("friendlyName", &v)),
            uptime: info.uptime,
            class: plan.class,
            warning: WARNING,
            token: plan.token,
            expires_in: plan.expires_in,
        })
    }

    /// Restarts the device with the token of its [plan](Self::plan_reboot),
    /// once. Reads the uptime first, to tell later whether the device
    /// restarted.
    ///
    /// # Errors
    ///
    /// The errors of the checks, [`Error::InvalidToken`], [`Error::State`]
    /// if the restart cannot be logged, and the errors of the call other
    /// than a broken connection, like a UPnP error.
    pub async fn reboot(&mut self, token: &str) -> Result<Reboot, Error> {
        let change = subject(SERVICE, ACTION, &[]);
        let checked = self.checked(change).await?;
        let (probe, before) = match Probe::first(&self.client).await {
            Ok(first) => first,
            Err(error) => return Err(self.refuse(change, error)),
        };
        let (outcome, error) = match self.send(&checked, change, token).await {
            Ok(service) => {
                // Logged as sending already, so a failed log loses nothing.
                let _ = self.log(Event::Applied, subject(&service, ACTION, &[]), None, None);
                (State::Accepted, None)
            }
            Err(Error::Call(generic::Error::Tr064(error @ tr064::Error::Http(_)))) => {
                (State::Unknown, Some(error.to_string()))
            }
            Err(error) => return Err(error),
        };
        let udn = self.identity.clone().map_or_default(|(_, udn)| udn);
        Ok(Reboot {
            report: Report {
                device: checked.identity,
                outcome,
                uptime_before: before.uptime,
                uptime: None,
                waited: None,
                error,
            },
            udn,
            probe,
        })
    }

    /// Waits until an [accepted](State::Accepted) restart is done: reads
    /// `DeviceInfo:GetInfo` every interval until the device answers with a
    /// lower uptime than before, or, if it reports none, answers again
    /// after it stopped. Calls `progress` with
    /// [rebooting](State::Rebooting) once it stops answering. Never sends
    /// the restart again.
    ///
    /// # Errors
    ///
    /// [`Error::OtherDevice`] if a device with another serial number comes
    /// back.
    pub async fn wait(
        &self,
        reboot: &Reboot,
        wait: Wait,
        mut progress: impl FnMut(State),
    ) -> Result<Report, Error> {
        let start = Instant::now();
        let deadline = start + wait.budget;
        let mut report = reboot.report.clone();
        let mut down = false;
        let mut left = wait.budget;
        while !left.is_zero() {
            tokio::time::sleep(wait.interval.min(left)).await;
            left = deadline.saturating_duration_since(Instant::now());
            let read = reboot.probe.read(&self.client);
            let Ok(Ok(info)) = tokio::time::timeout(left, read).await else {
                if !down {
                    down = true;
                    progress(State::Rebooting);
                }
                continue;
            };
            let restarted = info
                .uptime
                .zip(report.uptime_before)
                .map_or(down, |(now, before)| now < before);
            if restarted {
                let serial = info.serial.unwrap_or_default();
                let identity = Identity::of(&reboot.udn, &serial);
                if identity != report.device {
                    return Err(Error::OtherDevice(identity));
                }
                report.outcome = State::Back;
                report.uptime = info.uptime;
                report.waited = Some(start.elapsed().as_secs());
                return Ok(report);
            }
        }
        report.outcome = State::Timeout;
        report.waited = Some(start.elapsed().as_secs());
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_explains_the_states() {
        let states = [
            (State::Accepted, "accepted"),
            (State::Rebooting, "rebooting"),
            (State::Back, "back"),
            (State::Timeout, "timeout"),
            (State::Unknown, "unknown"),
        ];
        for (state, name) in states {
            assert_eq!(state.to_string(), name);
            assert_eq!(
                serde_json::to_value(state).unwrap(),
                serde_json::json!(name)
            );
            assert_ne!(state.meaning(), "");
        }
        assert_eq!(Wait::default().budget, Duration::from_secs(300));
    }
}
