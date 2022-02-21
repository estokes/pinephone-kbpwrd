use anyhow::{bail, Error, Result};
use log::{error, info};
use std::{
    cmp::{max, min},
    path::PathBuf,
    str::FromStr,
    time::{Duration, Instant},
};
use tokio::{fs, time};

async fn read<T>(path: &PathBuf) -> Result<std::result::Result<T, <T as FromStr>::Err>>
where
    T: FromStr,
{
    Ok(fs::read_to_string(path).await?.trim().parse::<T>())
}

#[derive(Debug, Clone, Copy)]
enum Model {
    PinePhone,
    PinePhonePro,
}

impl Model {
    // valid values that can be written to input_current_limit
    fn valid_limits(&self) -> &'static [u32] {
        static PPP: [u32; 10] = [
            80000, 450000, 850000, 1000000, 1250000, 1500000, 2000000, 2250000, 2500000, 3000000,
        ];
        //    static PP: ?
        match self {
            Model::PinePhonePro => &PPP,
            Model::PinePhone => unimplemented!(),
        }
    }

    // return the default input current limit
    fn default_limit(&self) -> u32 {
        match self {
            Model::PinePhonePro => self.valid_limits()[1],
            Model::PinePhone => unimplemented!(),
        }
    }

    // return the default input current limit
    fn max_limit(&self) -> u32 {
        match self {
            Model::PinePhonePro => self.valid_limits()[9],
            Model::PinePhone => unimplemented!(),
        }
    }

    // given the current input_curent_limit, step one increment up or down and return the new value
    fn limit_step(&self, up: bool, cur: u32) -> u32 {
        let valid = self.valid_limits();
        for (i, v) in valid.iter().enumerate() {
            if *v == cur {
                if up {
                    return valid[min(valid.len() - 1, i + 1)];
                } else if i == 0 {
                    return valid[0];
                } else {
                    return valid[i - 1];
                }
            }
        }
        valid[2]
    }
}

struct Device {
    model: Model,
    kb_state: PathBuf,
    kb_voltage: PathBuf,
    kb_current: PathBuf,
    kb_limit: PathBuf,
    kb_enabled: PathBuf,
    mb_state: PathBuf,
    mb_voltage: PathBuf,
    mb_current: PathBuf,
    mb_limit: PathBuf,
}

impl Device {
    fn new(model: Model) -> Device {
        let base = PathBuf::from("/sys/class/power_supply");
        Device {
            model,
            kb_current: base.join("ip5xxx-charger/current_now"),
            kb_voltage: base.join("ip5xxx-charger/voltage_now"),
            kb_state: base.join("ip5xxx-charger/status"),
            kb_limit: base.join("ip5xxx-charger/constant_charge_current"),
            kb_enabled: base.join("ip5xxx-boost/online"),
            mb_state: base.join("battery/status"),
            mb_voltage: base.join("battery/voltage_now"),
            mb_current: base.join("battery/current_now"),
            mb_limit: base.join("rk818-usb/input_current_limit"),
        }
    }

    async fn set_limit(&self, limit: u32) -> Result<()> {
        info!("setting input_current_limit: {}", limit / 1000);
        Ok(fs::write(&self.mb_limit, &format!("{}\n", limit)).await?)
    }

    async fn set_limit_step(&self, up: bool, cur: u32) -> Result<()> {
        let limit = self.model.limit_step(up, cur);
        Ok(if limit != cur {
            self.set_limit(limit).await?
        })
    }

    async fn set_limit_default(&self, cur: u32) -> Result<()> {
        let def = self.model.default_limit();
        Ok(if cur != def {
            self.set_limit(def).await?
        })
    }

    async fn set_limit_max(&self, cur: u32) -> Result<()> {
        let def = self.model.max_limit();
        Ok(if cur != def {
            self.set_limit(def).await?
        })
    }

    async fn info(&self) -> Result<Info> {
        Ok(Info {
            kbd: KeyboardBattery::get(self).await?,
            mb: MainBattery::get(self).await?,
        })
    }
}

#[derive(Debug)]
enum State {
    Charging,
    Discharging,
    Full,
}

impl FromStr for State {
    type Err = Error;

    fn from_str(s: &str) -> Result<State> {
        match s {
            "Charging" => Ok(State::Charging),
            "Discharging" => Ok(State::Discharging),
            "Full" | "Not charging" => Ok(State::Full),
            s => bail!("unexpected state {}", s),
        }
    }
}

#[derive(Debug)]
struct KeyboardBattery {
    state: State,
    voltage: i32,
    current: i32,
    limit: u32,
    enabled: bool,
}

impl KeyboardBattery {
    async fn get(dev: &Device) -> Result<KeyboardBattery> {
        Ok(KeyboardBattery {
            state: read::<State>(&dev.kb_state).await??,
            voltage: read::<i32>(&dev.kb_voltage).await??,
            current: read::<i32>(&dev.kb_current).await??,
            limit: read::<u32>(&dev.kb_limit).await??,
            enabled: read::<i32>(&dev.kb_enabled).await?? == 1,
        })
    }
}

#[derive(Debug)]
struct MainBattery {
    state: State,
    voltage: i32,
    current: i32,
    limit: u32,
}

impl MainBattery {
    async fn get(dev: &Device) -> Result<MainBattery> {
        let current = read::<i32>(&dev.mb_current).await??;
        let state = match read::<State>(&dev.mb_state).await?? {
            State::Full => State::Full,
            State::Charging if current > 0 => State::Charging,
            State::Charging => State::Discharging,
            State::Discharging => State::Discharging
        };
        Ok(MainBattery {
            state, current,
            voltage: read::<i32>(&dev.mb_voltage).await??,
            limit: read::<u32>(&dev.mb_limit).await??,
        })
    }
}

#[derive(Debug)]
struct Info {
    kbd: KeyboardBattery,
    mb: MainBattery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    MaybeStepUp,
    MaybeStepDown,
    StepUp,
    StepDown,
    SetDefault,
    SetMax,
    Pass,
}

async fn step(dev: &Device, kb_charging: &mut bool, last_step: &mut Instant) -> Result<()> {
    const STEP: Duration = Duration::from_secs(10);
    let info = dev.info().await?;
    let action = match info.kbd.state {
        State::Charging => {
            if !*kb_charging {
                *kb_charging = true;
                Action::SetDefault
            } else {
                let tot = info.kbd.current + max(0, info.mb.current);
                if tot < (info.kbd.limit - (info.kbd.limit / 5)) as i32 {
                    Action::MaybeStepUp
                } else if tot >= info.kbd.limit as i32 {
                    Action::SetDefault
                } else {
                    Action::Pass
                }
            }
        }
        State::Full => {
            if info.kbd.enabled {
                Action::SetMax
            } else {
                Action::SetDefault
            }
        }
        State::Discharging => {
            if *kb_charging {
                *kb_charging = false;
                Action::SetDefault
            } else {
                match info.mb.state {
                    State::Full => Action::SetDefault,
                    State::Charging => Action::MaybeStepDown,
                    State::Discharging => {
                        let mb = info.mb.current.abs();
                        let kb = info.kbd.current.abs();
                        if mb > kb {
                            Action::MaybeStepUp
                        } else {
                            Action::Pass
                        }
                    }
                }
            }
        }
    };
    info!(
        "ph v: {}, c: {}, s: {:?}, l: {}, kb v: {}, c: {}, s: {:?}, l: {}, act: {:?}",
        info.mb.voltage / 1000,
        info.mb.current / 1000,
        info.mb.state,
        info.mb.limit / 1000,
        info.kbd.voltage / 1000,
        info.kbd.current / 1000,
        info.kbd.state,
        info.kbd.limit / 1000,
        action
    );
    match action {
        Action::Pass => (),
        Action::MaybeStepUp | Action::StepUp => {
            if (action == Action::StepUp || last_step.elapsed() > STEP)
                && info.mb.limit < info.kbd.limit
            {
                *last_step = Instant::now();
                dev.set_limit_step(true, info.mb.limit).await?;
            }
        }
        Action::MaybeStepDown | Action::StepDown => {
            if action == Action::StepDown || last_step.elapsed() > STEP {
                *last_step = Instant::now();
                dev.set_limit_step(false, info.mb.limit).await?;
            }
        }
        Action::SetDefault => {
            *last_step = Instant::now();
            dev.set_limit_default(info.mb.limit).await?
        }
        Action::SetMax => {
            *last_step = Instant::now();
            dev.set_limit_max(info.mb.limit).await?
        }
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    env_logger::init();
    let dev = Device::new(Model::PinePhonePro);
    let mut kb_charging = false;
    let mut last_step = Instant::now();
    loop {
        time::sleep(Duration::from_secs(1)).await;
        if let Err(e) = step(&dev, &mut kb_charging, &mut last_step).await {
            error!("error: {} will retry", e);
        }
    }
}
