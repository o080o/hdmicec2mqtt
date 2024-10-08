use log::{debug, info, log_enabled, trace};
use std::process::Command;
use std::sync::{Arc, Mutex};

use crate::ha_entity::SimpleCommand;
use crate::process::CommandProcess;
use crate::service::StateManager;

pub trait ClonableHdmiCecProcess {
    fn command<F: 'static + Fn(&HdmiCecProcess, &str) -> ()>(&self, func: F) -> SimpleCommand;
}

impl ClonableHdmiCecProcess for Arc<HdmiCecProcess> {
    fn command<F: 'static + Fn(&HdmiCecProcess, &str) -> ()>(&self, func: F) -> SimpleCommand {
        let hdmicec = self.clone();
        return SimpleCommand::new(move |payload| {
            return func(&hdmicec, payload);
        });
    }
}

pub struct HdmiCecProcess {
    process: Mutex<CommandProcess>,
    state: Mutex<Option<StateManager>>,
    tv_state: Arc<Mutex<Option<String>>>,
}

impl HdmiCecProcess {
    pub fn new() -> Self {
        #[cfg(test)] // for testing, just use a dummy command, like cat!
        let mut command = Command::new("cat");
        #[cfg(not(test))] // in a real build, use cec-client
        let mut command = Command::new("cec-client");

        if !log_enabled!(log::Level::Trace) {
            command.arg("-d").arg("1");
        }

        let process = CommandProcess::new(&mut command);
        return Self {
            process: Mutex::new(process),
            state: Mutex::new(None),
            tv_state: Arc::new(Mutex::new(None)),
        };
    }

    pub fn attach_statemanager(&self, statemanager: StateManager) {
        self.state
            .lock()
            .expect("could not get lock")
            .replace(statemanager);
    }

    fn parse_power_state(line: &str) -> Option<String> {
        if line.starts_with("power status:") {
            let state_string = &line[14..];
            let mqtt_state = match state_string {
                "on" => "ON",
                "standby" => "OFF",
                _ => "UNKNOWN",
            };
            debug!("parsed power status: {mqtt_state} from section {state_string} of line {line}");
            return Some(mqtt_state.to_string());
        } else {
            trace!("cec-client >>>>> {}", line);
            return None;
        }
    }

    pub fn update_state(&self, state: bool) {
        let mqtt_state = if state { "ON" } else { "OFF" };

        self.tv_state
            .lock()
            .expect("could not get lock")
            .replace(mqtt_state.to_string());
        self.state
            .lock()
            .expect("could not get lock")
            .as_ref()
            .map(|state| {
                state.update_state(mqtt_state.to_string());
            });
    }

    pub fn kill(&self) -> Result<(), std::io::Error> {
        let mut process = self.process.lock().expect("could not lock process");
        return process.kill();
    }

    pub fn listen(&self) {
        info!("listening to the cec-client process...");
        let state = self
            .state
            .lock()
            .expect("could not lock state")
            .clone()
            .expect("no state manager present while listening.");
        let tv_state = self.tv_state.clone();
        let mut process = self.process.lock().expect("could not lock process");

        process
            .with_output(move |line| {
                trace!("got line from stdout: {}", line);
                if let Some(mqtt_state) = HdmiCecProcess::parse_power_state(&line) {
                    tv_state
                        .lock()
                        .expect("could not get lock")
                        .replace(mqtt_state.to_string());
                    state.update_state(mqtt_state.to_string());
                }
            })
            .expect("could not start listening process");
    }

    pub fn volume_up(&self) {
        let mut process = self.process.lock().expect("could not lock process");
        process.send("volup\n").unwrap();
    }

    pub fn set_tv(&self, state: bool) {
        let mut process = self.process.lock().expect("could not lock process");
        if state {
            process.send("on 0.0.0.0\n").unwrap();
        } else {
            process.send("standby 0.0.0.0\n").unwrap();
        }
        self.update_state(state);
    }

    pub fn volume_down(&self) {
        let mut process = self.process.lock().expect("could not lock process");
        process.send("voldown\n").unwrap();
    }

    pub fn mute(&self) {
        let mut process = self.process.lock().expect("could not lock process");
        process.send("mute\n").unwrap();
    }

    pub fn query_tv_state(&self) {
        let mut process = self.process.lock().expect("could not lock process");
        process.send("pow 0.0.0.0\n").unwrap();
    }

    pub fn set_active_source(&self, source: usize) {
        // not the best way to create the CEC frame.. but it works. taken from cec-o-matic at https://www.cec-o-matic.com/
        let mut process = self.process.lock().expect("could not lock process");
        process.send(&format!("tx 1F:82:{}0:00\n", source)).unwrap();
    }
}

#[test]
fn creating_hdmi_cec_process() {
    HdmiCecProcess::new();
}

#[test]
fn hdmi_cec_process_functions() {
    let cec = HdmiCecProcess::new();
    assert!(cec.state.lock().expect("could not take lock").is_none());

    let statemanager = StateManager::faux();
    cec.attach_statemanager(statemanager);
    assert!(cec.state.lock().expect("could not take lock").is_some());
}

#[test]
fn parsing_power_line() {
    assert_eq!(
        HdmiCecProcess::parse_power_state("power status: on"),
        Some("ON".to_string())
    );
    assert_eq!(
        HdmiCecProcess::parse_power_state("power status: standby"),
        Some("OFF".to_string())
    );
    assert_eq!(
        HdmiCecProcess::parse_power_state("power status: idk"),
        Some("UNKNOWN".to_string())
    );

    assert_eq!(HdmiCecProcess::parse_power_state("random junk"), None);
}
