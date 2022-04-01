use anyhow::{bail, Error, Result};
use log::{error, info};
use std::{
    cmp::min,
    future::Future,
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
    fn detect() -> Result<Self> {
        if PathBuf::from("/sys/class/power_supply/rk818-usb").exists() {
            Ok(Model::PinePhonePro)
        } else if PathBuf::from("/sys/class/power_supply/axp20x-usb").exists() {
            Ok(Model::PinePhone)
        } else {
            bail!("unknown model")
        }
    }

    // valid values that can be written to input_current_limit
    fn valid_limits(&self) -> &'static [u32] {
        static PPP: [u32; 6] = [450000, 850000, 1000000, 1250000, 1500000, 2000000];
        static PP: [u32; 4] = [500000, 900000, 1500000, 2000000];
        match self {
            Model::PinePhonePro => &PPP,
            Model::PinePhone => &PP,
        }
    }

    // return the default input current limit
    fn default_limit(&self) -> u32 {
        match self {
            Model::PinePhonePro => self.valid_limits()[0],
            Model::PinePhone => self.valid_limits()[0],
        }
    }

    // return the max input current limit
    fn max_limit(&self) -> u32 {
        match self {
            Model::PinePhonePro => self.valid_limits()[5],
            Model::PinePhone => self.valid_limits()[3],
        }
    }

    fn min_limit(&self) -> u32 {
        self.valid_limits()[0]
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
    kb_soc: PathBuf,
    kb_state: PathBuf,
    kb_voltage: PathBuf,
    kb_current: PathBuf,
    kb_limit: PathBuf,
    kb_enabled: PathBuf,
    mb_state: PathBuf,
    mb_soc: PathBuf,
    mb_voltage: PathBuf,
    mb_current: PathBuf,
    mb_limit: PathBuf,
}

impl Device {
    fn new(model: Model) -> Device {
        let base = PathBuf::from("/sys/class/power_supply");
        match model {
            Model::PinePhonePro => Device {
                model,
                kb_current: base.join("ip5xxx-charger/current_now"),
                kb_voltage: base.join("ip5xxx-charger/voltage_now"),
                kb_soc: base.join("ip5xxx-charger/capacity"),
                kb_state: base.join("ip5xxx-charger/status"),
                kb_limit: base.join("ip5xxx-charger/constant_charge_current"),
                kb_enabled: base.join("ip5xxx-boost/online"),
                mb_state: base.join("battery/status"),
                mb_soc: base.join("battery/capacity"),
                mb_voltage: base.join("battery/voltage_now"),
                mb_current: base.join("battery/current_now"),
                mb_limit: base.join("rk818-usb/input_current_limit"),
            },
            Model::PinePhone => Device {
                model,
                kb_current: base.join("ip5xxx-charger/current_now"),
                kb_voltage: base.join("ip5xxx-charger/voltage_now"),
                kb_soc: base.join("ip5xxx-charger/capacity"),
                kb_state: base.join("ip5xxx-charger/status"),
                kb_limit: base.join("ip5xxx-charger/constant_charge_current"),
                kb_enabled: base.join("ip5xxx-boost/online"),
                mb_state: base.join("axp20x-battery/status"),
                mb_soc: base.join("axp20x-battery/capacity"),
                mb_voltage: base.join("axp20x-battery/voltage_now"),
                mb_current: base.join("axp20x-battery/current_now"),
                mb_limit: base.join("axp20x-usb/input_current_limit"),
            },
        }
    }

    async fn set_online(&self, desired: bool, cur: bool) -> Result<()> {
        if desired != cur {
            info!("setting online: {}", desired);
            let desired = if desired { "1" } else { "0" };
            Ok(fs::write(&self.kb_enabled, desired).await?)
        } else {
            Ok(())
        }
    }

    async fn set_limit(&self, limit: u32) -> Result<()> {
        info!("setting input_current_limit: {}", limit / 1000);
        Ok(fs::write(&self.mb_limit, &format!("{}\n", limit)).await?)
    }

    async fn set_kb_limit(&self, limit: u32) -> Result<()> {
        info!("setting kb input_current_limit: {}", limit / 1000);
        Ok(fs::write(&self.kb_limit, &format!("{}\n", limit)).await?)
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
    soc: Option<u32>,
    voltage: u32,
    current: i32,
    limit: u32,
    enabled: bool,
}

impl KeyboardBattery {
    async fn get(dev: &Device) -> Result<KeyboardBattery> {
        Ok(KeyboardBattery {
            state: read(&dev.kb_state).await??,
            soc: read(&dev.kb_soc).await.ok().and_then(|v| v.ok()),
            voltage: read(&dev.kb_voltage).await??,
            current: read(&dev.kb_current).await??,
            limit: read(&dev.kb_limit).await??,
            enabled: read::<i32>(&dev.kb_enabled).await?? == 1,
        })
    }
}

#[derive(Debug)]
struct MainBattery {
    state: State,
    soc: u32,
    voltage: u32,
    current: i32,
    limit: u32,
}

impl MainBattery {
    async fn get(dev: &Device) -> Result<MainBattery> {
        async fn get_state(dev: &Device, current: i32) -> Result<State> {
            Ok(match read(&dev.mb_state).await?? {
                State::Full => State::Full,
                State::Charging if current > 0 => State::Charging,
                State::Charging => State::Discharging,
                State::Discharging => State::Discharging,
            })
        }
        match dev.model {
            Model::PinePhonePro => {
                let current: i32 = read(&dev.mb_current).await??;
                Ok(MainBattery {
                    state: get_state(dev, current).await?,
                    soc: read(&dev.mb_soc).await??,
                    current,
                    voltage: read(&dev.mb_voltage).await??,
                    limit: read(&dev.mb_limit).await??,
                })
            }
            Model::PinePhone => {
                let limit: u32 = read(&dev.mb_limit).await??;
                let current_abs: i32 = read(&dev.mb_current).await??;
                // this hack works around a kernel bug that causes
                // only abs(current) to be reported. It isnt't
                // perfect, but it catches the obvious cases.
                let current = if current_abs > ((limit as i32) + (limit as i32 >> 2)) {
                    !current_abs
                } else {
                    current_abs
                };
                Ok(MainBattery {
                    state: get_state(dev, current).await?,
                    soc: read(&dev.mb_soc).await??,
                    current,
                    voltage: read(&dev.mb_voltage).await??,
                    limit,
                })
            }
        }
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
    MaybePhUpKbDown,
    MaybeStepKbUp,
    SetDefault,
    SetMax,
    Pass,
}

struct Ctx {
    dev: Device,
    kb_charging: bool,
    last_step: Instant,
    last_offline: Instant,
}

const KBLIM: i32 = 2300000;

impl Ctx {
    fn decide(&mut self, info: &Info) -> Action {
        match info.kbd.state {
            State::Charging => {
                if !self.kb_charging {
                    self.kb_charging = true;
                    Action::SetDefault
                } else {
                    let lim = KBLIM + (KBLIM >> 4);
                    let ka = info.kbd.current;
                    let tot = ka + info.mb.limit as i32;
                    let nextl = self.dev.model.limit_step(true, info.mb.limit) as i32;
                    if ka + nextl < lim && info.mb.current < 0 {
                        Action::MaybeStepUp
                    } else if info.mb.current < 0 {
                        Action::MaybePhUpKbDown
                    } else if tot >= lim {
                        Action::MaybeStepDown
                    } else if tot < KBLIM {
                        Action::MaybeStepKbUp
                    } else {
                        Action::Pass
                    }
                }
            }
            State::Full => {
                if info.kbd.enabled && self.kb_charging {
                    Action::SetMax
                } else {
                    Action::SetDefault
                }
            }
            State::Discharging => {
                if self.kb_charging {
                    self.kb_charging = false;
                    Action::SetDefault
                } else {
                    match info.mb.state {
                        State::Full => Action::SetDefault,
                        State::Charging if info.mb.soc > 30 => Action::MaybeStepDown,
                        State::Discharging if info.mb.soc > 30 => {
                            const VDIF: u32 = 150000;
                            let mbv = info.mb.voltage;
                            let kbv = info.kbd.voltage;
                            let mbc = info.mb.current.abs();
                            let kbc = info.kbd.current.abs();
                            if mbv > kbv && mbv - kbv > VDIF {
                                Action::MaybeStepDown
                            } else if (mbv >= kbv && mbv - kbv < VDIF)
                                || (kbv >= mbv && kbv - mbv < VDIF)
                            {
                                Action::Pass
                            } else if mbc > kbc {
                                Action::MaybeStepUp
                            } else {
                                Action::Pass
                            }
                        }
                        // keep the main battery above 30% for as long as
                        // possible even if that means charging it.
                        State::Charging => {
                            let delta =
                                info.mb.limit - self.dev.model.limit_step(false, info.mb.limit);
                            if info.mb.current > 0 && delta < info.mb.current as u32 {
                                Action::MaybeStepDown
                            } else {
                                Action::Pass
                            }
                        }
                        State::Discharging => Action::MaybeStepUp,
                    }
                }
            }
        }
    }

    async fn maybe_step<'a, R: Future<Output = Result<()>>, F: FnOnce(&'a mut Ctx) -> R>(
        &'a mut self,
        f: F,
    ) -> Result<()> {
        const STEP: Duration = Duration::from_secs(10);
        if self.last_step.elapsed() > STEP {
            self.last_step = Instant::now();
            f(self).await?
        }
        Ok(())
    }

    async fn step_up(&mut self, info: &Info) -> Result<()> {
        if !info.kbd.enabled {
            self.dev.set_online(true, info.kbd.enabled).await?;
        } else {
            self.dev.set_limit_step(true, info.mb.limit).await?;
        }
        Ok(())
    }

    async fn step_down(&mut self, info: &Info) -> Result<()> {
        if info.mb.limit == self.dev.model.min_limit() {
            self.last_offline = Instant::now();
            self.dev.set_online(false, info.kbd.enabled).await?;
        } else {
            self.dev.set_limit_step(false, info.mb.limit).await?;
        }
        Ok(())
    }

    async fn step(&mut self) -> Result<()> {
        const OFFLINE: Duration = Duration::from_secs(20);
        let info = self.dev.info().await?;
        let action = self.decide(&info);
        info!(
            "ph v: {}, a: {}, s: {:?}, l: {}, c: {}, kb v: {}, a: {}, s: {:?}, l: {}, c: {}, act: {:?}",
            info.mb.voltage / 1000,
            info.mb.current / 1000,
            info.mb.state,
            info.mb.limit / 1000,
            info.mb.soc,
            info.kbd.voltage / 1000,
            info.kbd.current / 1000,
            info.kbd.state,
            info.kbd.limit / 1000,
            match info.kbd.soc {
                Some(v) => v.to_string(),
                None => "n/a".into(),
            },
            action
        );
        // if the boost is left offline too long we lose communication with it
        if !info.kbd.enabled && self.last_offline.elapsed() > OFFLINE {
            self.last_step = Instant::now();
            self.dev.set_online(true, info.kbd.enabled).await?;
        }
        match action {
            Action::Pass => (),
            Action::MaybeStepUp => {
                self.maybe_step(|ctx| async { ctx.step_up(&info).await })
                    .await?
            }
            Action::MaybePhUpKbDown => {
                self.maybe_step(|ctx| async {
                    ctx.dev.set_online(true, info.kbd.enabled).await?;
                    let lim = ctx.dev.model.limit_step(true, info.mb.limit);
                    ctx.dev.set_kb_limit(KBLIM as u32 - lim).await?;
                    ctx.dev.set_limit_step(true, info.mb.limit).await?;
                    Ok(())
                })
                .await?
            }
            Action::MaybeStepKbUp => {
                self.maybe_step(|ctx| async {
                    ctx.dev.set_online(true, info.kbd.enabled).await?;
                    if info.kbd.limit < KBLIM as u32 {
                        ctx.dev
                            .set_kb_limit(min(info.kbd.limit + 100000, KBLIM as u32))
                            .await?
                    }
                    Ok(())
                })
                .await?
            }
            Action::MaybeStepDown => {
                self.maybe_step(|ctx| async { ctx.step_down(&info).await })
                    .await?
            }
            Action::SetDefault => {
                self.last_step = Instant::now();
                self.dev.set_online(true, info.kbd.enabled).await?;
                self.dev.set_limit_default(info.mb.limit).await?;
                self.dev
                    .set_kb_limit(KBLIM as u32 - self.dev.model.default_limit())
                    .await?;
            }
            Action::SetMax => {
                self.last_step = Instant::now();
                self.dev.set_online(true, info.kbd.enabled).await?;
                self.dev.set_limit_max(info.mb.limit).await?;
                self.dev
                    .set_kb_limit(KBLIM as u32 - self.dev.model.default_limit())
                    .await?;
            }
        }
        Ok(())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    env_logger::init();
    let mut ctx = Ctx {
        dev: Device::new(Model::detect()?),
        kb_charging: false,
        last_step: Instant::now(),
        last_offline: Instant::now(),
    };
    loop {
        time::sleep(Duration::from_secs(1)).await;
        if let Err(e) = ctx.step().await {
            error!("error: {} will retry", e);
        }
    }
}
