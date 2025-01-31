/*
    This file is part of Eruption.

    Eruption is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation, either version 3 of the License, or
    (at your option) any later version.

    Eruption is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU General Public License for more details.

    You should have received a copy of the GNU General Public License
    along with Eruption.  If not, see <http://www.gnu.org/licenses/>.

    Copyright (c) 2019-2022, The Eruption Development Team
*/

use evdev_rs::enums::EV_SYN;
use evdev_rs::{Device, DeviceWrapper, GrabMode};
use flume::{unbounded, Receiver, Sender};
use log::{debug, error, info, trace, warn};
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::{
    constants, dbus_interface, hwdevices, macros, plugins, script, sdk_support, uleds, util,
    DeviceAction, EvdevError, KeyboardDevice, MainError, MouseDevice, Profile,
    COLOR_MAPS_READY_CONDITION, FAILED_TXS, KEY_STATES, LUA_TXS, QUIT, REQUEST_FAILSAFE_MODE, RGBA,
    SDK_SUPPORT_ACTIVE, ULEDS_SUPPORT_ACTIVE,
};

pub type Result<T> = std::result::Result<T, eyre::Error>;

#[derive(Debug, Clone)]
pub enum DbusApiEvent {
    ProfilesChanged,
    ActiveProfileChanged,
    ActiveSlotChanged,
    BrightnessChanged,
    DeviceStatusChanged,
    DeviceHotplug((u16, u16), bool),
}

/// Spawns the D-Bus API thread and executes it's main loop
pub fn spawn_dbus_api_thread(
    dbus_tx: Sender<dbus_interface::Message>,
) -> plugins::Result<Sender<DbusApiEvent>> {
    let (dbus_api_tx, dbus_api_rx) = unbounded();

    thread::Builder::new()
        .name("dbus-interface".into())
        .spawn(move || -> Result<()> {
            #[cfg(feature = "profiling")]
            coz::thread_init();

            let dbus = dbus_interface::initialize(dbus_tx)?;

            // will be set to true if we received a dbus event in the current iteration of the loop
            let mut event_received = false;

            loop {
                let timeout = if event_received { 0 } else { 5 };

                // process events, destined for the dbus api
                match dbus_api_rx.recv_timeout(Duration::from_millis(timeout)) {
                    Ok(result) => match result {
                        DbusApiEvent::ProfilesChanged => dbus.notify_profiles_changed()?,

                        DbusApiEvent::ActiveProfileChanged => {
                            dbus.notify_active_profile_changed()?
                        }

                        DbusApiEvent::ActiveSlotChanged => dbus.notify_active_slot_changed()?,

                        DbusApiEvent::BrightnessChanged => dbus.notify_brightness_changed()?,

                        DbusApiEvent::DeviceStatusChanged => dbus.notify_device_status_changed()?,

                        DbusApiEvent::DeviceHotplug(device_info, remove) => {
                            dbus.notify_device_hotplug(device_info, remove)?
                        }
                    },

                    Err(_e) => {
                        event_received = dbus.get_next_event_timeout(0).unwrap_or_else(|e| {
                            error!("Could not get the next D-Bus event: {}", e);

                            false
                        });
                    }
                };
            }
        })?;

    Ok(dbus_api_tx)
}

/// Spawns the keyboard events thread and executes it's main loop
pub fn spawn_keyboard_input_thread(
    kbd_tx: Sender<Option<evdev_rs::InputEvent>>,
    keyboard_device: KeyboardDevice,
    device_index: usize,
    usb_vid: u16,
    usb_pid: u16,
) -> plugins::Result<()> {
    thread::Builder::new()
        .name(format!("events/kbd:{}", device_index))
        .spawn(move || -> Result<()> {
            #[cfg(feature = "profiling")]
            coz::thread_init();

            let device = match hwdevices::get_input_dev_from_udev(usb_vid, usb_pid) {
                Ok(filename) => match File::open(filename.clone()) {
                    Ok(devfile) => match Device::new_from_file(devfile) {
                        Ok(mut device) => {
                            info!("Now listening on keyboard: {}", filename);

                            info!(
                                "Input device name: \"{}\"",
                                device.name().unwrap_or("<n/a>")
                            );

                            info!(
                                "Input device ID: bus 0x{:x} vendor 0x{:x} product 0x{:x}",
                                device.bustype(),
                                device.vendor_id(),
                                device.product_id()
                            );

                            // info!("Driver version: {:x}", device.driver_version());

                            info!("Physical location: {}", device.phys().unwrap_or("<n/a>"));

                            // info!("Unique identifier: {}", device.uniq().unwrap_or("<n/a>"));

                            info!("Grabbing the keyboard device exclusively");
                            let _ = device
                                .grab(GrabMode::Grab)
                                .map_err(|e| error!("Could not grab the device: {}", e));

                            device
                        }

                        Err(_e) => return Err(EvdevError::EvdevHandleError {}.into()),
                    },

                    Err(_e) => return Err(EvdevError::EvdevError {}.into()),
                },

                Err(_e) => return Err(EvdevError::UdevError {}.into()),
            };

            loop {
                // check if we shall terminate the input thread, before we poll the keyboard
                if QUIT.load(Ordering::SeqCst) {
                    break Ok(());
                }

                if keyboard_device.read().has_failed()? {
                    warn!("Terminating input thread due to a failed device");

                    // we need to terminate and then re-enter the main loop to update all global state
                    crate::REENTER_MAIN_LOOP.store(true, Ordering::SeqCst);

                    break Ok(());
                }

                match device.next_event(evdev_rs::ReadFlag::NORMAL | evdev_rs::ReadFlag::BLOCKING) {
                    Ok(k) => {
                        trace!("Key event: {:?}", k.1);

                        // reset "to be dropped" flag
                        macros::DROP_CURRENT_KEY.store(false, Ordering::SeqCst);

                        // update our internal representation of the keyboard state
                        if let evdev_rs::enums::EventCode::EV_KEY(ref code) = k.1.event_code {
                            let is_pressed = k.1.value > 0;
                            let index = keyboard_device.read().ev_key_to_key_index(*code) as usize;

                            KEY_STATES.write()[index] = is_pressed;
                        }

                        kbd_tx.send(Some(k.1)).unwrap_or_else(|e| {
                            error!("Could not send a keyboard event to the main thread: {}", e)
                        });

                        // update AFK timer
                        *crate::LAST_INPUT_TIME.lock() = Instant::now();
                    }

                    Err(e) => {
                        if e.raw_os_error().unwrap() == libc::ENODEV {
                            warn!("Keyboard device went away: {}", e);

                            // we need to terminate and then re-enter the main loop to update all global state
                            crate::REENTER_MAIN_LOOP.store(true, Ordering::SeqCst);

                            return Err(EvdevError::EvdevEventError {}.into());
                        } else {
                            error!("Could not peek evdev event: {}", e);

                            // we need to terminate and then re-enter the main loop to update all global state
                            crate::REENTER_MAIN_LOOP.store(true, Ordering::SeqCst);

                            return Err(EvdevError::EvdevEventError {}.into());
                        }
                    }
                };
            }
        })
        .unwrap_or_else(|e| {
            error!("Could not spawn a thread: {}", e);
            panic!()
        });

    Ok(())
}

/// Spawns the mouse events thread and executes it's main loop
pub fn spawn_mouse_input_thread(
    mouse_tx: Sender<Option<evdev_rs::InputEvent>>,
    mouse_device: MouseDevice,
    device_index: usize,
    usb_vid: u16,
    usb_pid: u16,
) -> plugins::Result<()> {
    thread::Builder::new()
        .name(format!("events/mouse:{}", device_index))
        .spawn(move || -> Result<()> {
            #[cfg(feature = "profiling")]
            coz::thread_init();

            let device = match hwdevices::get_input_dev_from_udev(usb_vid, usb_pid) {
                Ok(filename) => match File::open(filename.clone()) {
                    Ok(devfile) => match Device::new_from_file(devfile) {
                        Ok(mut device) => {
                            info!("Now listening on mouse: {}", filename);

                            info!(
                                "Input device name: \"{}\"",
                                device.name().unwrap_or("<n/a>")
                            );

                            info!(
                                "Input device ID: bus 0x{:x} vendor 0x{:x} product 0x{:x}",
                                device.bustype(),
                                device.vendor_id(),
                                device.product_id()
                            );

                            // info!("Driver version: {:x}", device.driver_version());

                            info!("Physical location: {}", device.phys().unwrap_or("<n/a>"));

                            // info!("Unique identifier: {}", device.uniq().unwrap_or("<n/a>"));

                            info!("Grabbing the mouse device exclusively");
                            let _ = device
                                .grab(GrabMode::Grab)
                                .map_err(|e| error!("Could not grab the device: {}", e));

                            device
                        }

                        Err(_e) => return Err(EvdevError::EvdevHandleError {}.into()),
                    },

                    Err(_e) => return Err(EvdevError::EvdevError {}.into()),
                },

                Err(_e) => return Err(EvdevError::UdevError {}.into()),
            };

            loop {
                // check if we shall terminate the input thread, before we poll the mouse device
                if QUIT.load(Ordering::SeqCst) {
                    break Ok(());
                }

                match device.next_event(evdev_rs::ReadFlag::NORMAL | evdev_rs::ReadFlag::BLOCKING) {
                    Ok(k) => {
                        // trace!("Mouse event: {:?}", k.1);

                        // reset "to be dropped" flag
                        macros::DROP_CURRENT_MOUSE_INPUT.store(false, Ordering::SeqCst);

                        // update our internal representation of the device state
                        if let evdev_rs::enums::EventCode::EV_SYN(code) = k.1.clone().event_code {
                            if code == EV_SYN::SYN_DROPPED {
                                warn!("Mouse:{} dropped some events, resyncing...", device_index);
                                device.next_event(evdev_rs::ReadFlag::SYNC)?;
                            } else {
                                // directly mirror SYN events to reduce input lag
                                if crate::GRAB_MOUSE.load(Ordering::SeqCst) {
                                    macros::UINPUT_TX
                                        .read()
                                        .as_ref()
                                        .unwrap()
                                        .send(macros::Message::MirrorMouseEventImmediate(
                                            k.1.clone(),
                                        ))
                                        .unwrap_or_else(|e| {
                                            error!("Could not send a pending mouse event: {}", e)
                                        });
                                }
                            }
                        } else if let evdev_rs::enums::EventCode::EV_KEY(code) =
                            k.1.clone().event_code
                        {
                            let is_pressed = k.1.value > 0;
                            match mouse_device.read().ev_key_to_button_index(code) {
                                Ok(index) => {
                                    crate::BUTTON_STATES.write()[index as usize] = is_pressed
                                }

                                Err(e) => {
                                    log::warn!("Mouse event for '{code:?}' not processed: {e}")
                                }
                            }
                        } else if let evdev_rs::enums::EventCode::EV_REL(code) =
                            k.1.clone().event_code
                        {
                            // ignore mouse wheel related events here
                            if code != evdev_rs::enums::EV_REL::REL_WHEEL
                                && code != evdev_rs::enums::EV_REL::REL_HWHEEL
                                && code != evdev_rs::enums::EV_REL::REL_WHEEL_HI_RES
                                && code != evdev_rs::enums::EV_REL::REL_HWHEEL_HI_RES
                            {
                                // directly mirror pointer motion events to reduce input lag.
                                // This currently prohibits further manipulation of pointer motion events
                                if crate::GRAB_MOUSE.load(Ordering::SeqCst) {
                                    macros::UINPUT_TX
                                        .read()
                                        .as_ref()
                                        .unwrap()
                                        .send(macros::Message::MirrorMouseEventImmediate(
                                            k.1.clone(),
                                        ))
                                        .unwrap_or_else(|e| {
                                            error!("Could not send a pending mouse event: {}", e)
                                        });
                                }
                            }
                        }

                        mouse_tx.send(Some(k.1)).unwrap_or_else(|e| {
                            error!("Could not send a mouse event to the main thread: {}", e)
                        });

                        // update AFK timer
                        *crate::LAST_INPUT_TIME.lock() = Instant::now();
                    }

                    Err(e) => {
                        if e.raw_os_error().unwrap() == libc::ENODEV {
                            warn!("Mouse device went away: {}", e);

                            // we need to terminate and then re-enter the main loop to update all global state
                            crate::REENTER_MAIN_LOOP.store(true, Ordering::SeqCst);

                            return Err(EvdevError::EvdevEventError {}.into());
                        } else {
                            error!("Could not peek evdev event: {}", e);

                            // we need to terminate and then re-enter the main loop to update all global state
                            crate::REENTER_MAIN_LOOP.store(true, Ordering::SeqCst);

                            return Err(EvdevError::EvdevEventError {}.into());
                        }
                    }
                };
            }
        })
        .unwrap_or_else(|e| {
            error!("Could not spawn a thread: {}", e);
            panic!()
        });

    Ok(())
}

/// Spawns the mouse events thread for an additional sub-device on the mouse and executes the thread's main loop
/* pub fn spawn_mouse_input_thread_secondary(
    mouse_tx: Sender<Option<evdev_rs::InputEvent>>,
    mouse_device: MouseDevice,
    device_index: usize,
    usb_vid: u16,
    usb_pid: u16,
) -> plugins::Result<()> {
    thread::Builder::new()
        .name(format!("events/mouse-sub:{}", device_index))
        .spawn(move || -> Result<()> {
            let device = match hwdevices::get_input_sub_dev_from_udev(usb_vid, usb_pid, 2) {
                Ok(filename) => match File::open(filename.clone()) {
                    Ok(devfile) => match Device::new_from_file(devfile) {
                        Ok(mut device) => {
                            info!("Now listening on mouse sub-dev: {}", filename);

                            info!(
                                "Input device name: \"{}\"",
                                device.name().unwrap_or("<n/a>")
                            );

                            info!(
                                "Input device ID: bus 0x{:x} vendor 0x{:x} product 0x{:x}",
                                device.bustype(),
                                device.vendor_id(),
                                device.product_id()
                            );

                            // info!("Driver version: {:x}", device.driver_version());

                            info!("Physical location: {}", device.phys().unwrap_or("<n/a>"));

                            // info!("Unique identifier: {}", device.uniq().unwrap_or("<n/a>"));

                            info!("Grabbing the sub-device exclusively");
                            let _ = device
                                .grab(GrabMode::Grab)
                                .map_err(|e| error!("Could not grab the device: {}", e));

                            device
                        }

                        Err(_e) => return Err(EvdevError::EvdevHandleError {}.into()),
                    },

                    Err(_e) => return Err(EvdevError::EvdevError {}.into()),
                },

                Err(_e) => return Err(EvdevError::UdevError {}.into()),
            };

            loop {
                // check if we shall terminate the input thread, before we poll the mouse device
                if QUIT.load(Ordering::SeqCst) {
                    break Ok(());
                }

                match device.next_event(evdev_rs::ReadFlag::NORMAL | evdev_rs::ReadFlag::BLOCKING) {
                    Ok(k) => {
                        trace!("Mouse sub-device event: {:?}", k.1);

                        // reset "to be dropped" flag
                        macros::DROP_CURRENT_MOUSE_INPUT.store(false, Ordering::SeqCst);

                        // update our internal representation of the device state
                        if let evdev_rs::enums::EventCode::EV_SYN(code) = k.1.clone().event_code {
                            if code == EV_SYN::SYN_DROPPED {
                                warn!("Mouse-sub:{} dropped some events, resyncing...", device_index);
                                device.next_event(evdev_rs::ReadFlag::SYNC)?;
                            } else {
                                // directly mirror SYN events to reduce input lag
                                if GRAB_MOUSE.load(Ordering::SeqCst) {
                                    macros::UINPUT_TX
                                        .read()
                                        .as_ref()
                                        .unwrap()
                                        .send(macros::Message::MirrorMouseEventImmediate(
                                            k.1.clone(),
                                        ))
                                        .unwrap_or_else(|e| {
                                            error!("Could not send a pending mouse event: {}", e)
                                        });
                                }
                            }
                        } else if let evdev_rs::enums::EventCode::EV_KEY(code) = k.1.clone().event_code {
                            let is_pressed = k.1.value > 0;
                            let index = mouse_device.read().ev_key_to_button_index(code).unwrap() as usize;

                            BUTTON_STATES.write()[index] = is_pressed;
                        } else if let evdev_rs::enums::EventCode::EV_REL(code) =
                            k.1.clone().event_code
                        {
                            if code != evdev_rs::enums::EV_REL::REL_WHEEL
                                && code != evdev_rs::enums::EV_REL::REL_HWHEEL
                                && code != evdev_rs::enums::EV_REL::REL_WHEEL_HI_RES
                                && code != evdev_rs::enums::EV_REL::REL_HWHEEL_HI_RES
                            {
                                // directly mirror pointer motion events to reduce input lag.
                                // This currently prohibits further manipulation of pointer motion events
                                if GRAB_MOUSE.load(Ordering::SeqCst) {
                                    macros::UINPUT_TX
                                        .read()
                                        .as_ref()
                                        .unwrap()
                                        .send(macros::Message::MirrorMouseEventImmediate(
                                            k.1.clone(),
                                        ))
                                        .unwrap_or_else(|e| {
                                            error!("Could not send a pending mouse sub-device event: {}", e)
                                        });
                                }
                            }
                        }

                        mouse_tx.send(Some(k.1)).unwrap_or_else(|e| {
                            error!("Could not send a mouse sub-device event to the main thread: {}", e)
                        });

                        // update AFK timer
                        *crate::LAST_INPUT_TIME.lock() = Instant::now();
                    }

                    Err(e) => {
                        if e.raw_os_error().unwrap() == libc::ENODEV {
                            warn!("Mouse sub-device went away: {}", e);

                            // we need to terminate and then re-enter the main loop to update all global state
                            crate::REENTER_MAIN_LOOP
                            .store(true, Ordering::SeqCst);

                            return Err(EvdevError::EvdevEventError {}.into());
                        } else {
                            error!("Could not peek evdev event: {}", e);

                            // we need to terminate and then re-enter the main loop to update all global state
                            crate::REENTER_MAIN_LOOP
                            .store(true, Ordering::SeqCst);

                            return Err(EvdevError::EvdevEventError {}.into());
                        }
                    }
                };
            }
        })
        .unwrap_or_else(|e| {
            error!("Could not spawn a thread: {}", e);
            panic!()
        });

    Ok(())
} */

/// Spawns the misc devices input thread and executes it's main loop
pub fn spawn_misc_input_thread(
    misc_tx: Sender<Option<evdev_rs::InputEvent>>,
    _misc_device: crate::MiscDevice,
    device_index: usize,
    usb_vid: u16,
    usb_pid: u16,
) -> plugins::Result<()> {
    thread::Builder::new()
        .name(format!("events/misc:{}", device_index))
        .spawn(move || -> Result<()> {
            #[cfg(feature = "profiling")]
            coz::thread_init();

            let device = match hwdevices::get_input_dev_from_udev(usb_vid, usb_pid) {
                Ok(filename) => match File::open(filename.clone()) {
                    Ok(devfile) => match Device::new_from_file(devfile) {
                        Ok(mut device) => {
                            info!("Now listening on misc device input: {}", filename);

                            info!(
                                "Input device name: \"{}\"",
                                device.name().unwrap_or("<n/a>")
                            );

                            info!(
                                "Input device ID: bus 0x{:x} vendor 0x{:x} product 0x{:x}",
                                device.bustype(),
                                device.vendor_id(),
                                device.product_id()
                            );

                            // info!("Driver version: {:x}", device.driver_version());

                            info!("Physical location: {}", device.phys().unwrap_or("<n/a>"));

                            // info!("Unique identifier: {}", device.uniq().unwrap_or("<n/a>"));

                            info!("Grabbing the misc device input exclusively");
                            let _ = device
                                .grab(GrabMode::Grab)
                                .map_err(|e| error!("Could not grab the device: {}", e));

                            device
                        }

                        Err(_e) => return Err(EvdevError::EvdevHandleError {}.into()),
                    },

                    Err(_e) => return Err(EvdevError::EvdevError {}.into()),
                },

                Err(_e) => return Err(EvdevError::UdevError {}.into()),
            };

            loop {
                // check if we shall terminate the input thread, before we poll the device
                if QUIT.load(Ordering::SeqCst) {
                    break Ok(());
                }

                match device.next_event(evdev_rs::ReadFlag::NORMAL | evdev_rs::ReadFlag::BLOCKING) {
                    Ok(k) => {
                        trace!("Misc event: {:?}", k.1);

                        // reset "to be dropped" flag
                        // macros::DROP_CURRENT_MISC_INPUT.store(false, Ordering::SeqCst);

                        // directly mirror pointer motion events to reduce input lag.
                        // This currently prohibits further manipulation of pointer motion events
                        macros::UINPUT_TX
                            .read()
                            .as_ref()
                            .unwrap()
                            .send(macros::Message::MirrorKey(k.1.clone()))
                            .unwrap_or_else(|e| {
                                error!("Could not send a pending misc device input event: {}", e)
                            });

                        misc_tx.send(Some(k.1)).unwrap_or_else(|e| {
                            error!(
                                "Could not send a misc device input event to the main thread: {}",
                                e
                            )
                        });

                        // update AFK timer
                        *crate::LAST_INPUT_TIME.lock() = Instant::now();
                    }

                    Err(e) => {
                        if e.raw_os_error().unwrap() == libc::ENODEV {
                            warn!("Misc device went away: {}", e);

                            // we need to terminate and then re-enter the main loop to update all global state
                            crate::REENTER_MAIN_LOOP.store(true, Ordering::SeqCst);

                            return Err(EvdevError::EvdevEventError {}.into());
                        } else {
                            error!("Could not peek evdev event: {}", e);

                            // we need to terminate and then re-enter the main loop to update all global state
                            crate::REENTER_MAIN_LOOP.store(true, Ordering::SeqCst);

                            return Err(EvdevError::EvdevEventError {}.into());
                        }
                    }
                };
            }
        })
        .unwrap_or_else(|e| {
            error!("Could not spawn a thread: {}", e);
            panic!()
        });

    Ok(())
}

pub fn spawn_lua_thread(
    thread_idx: usize,
    lua_rx: Receiver<script::Message>,
    script_path: PathBuf,
    profile: Option<Profile>,
) -> Result<()> {
    info!("Loading Lua script: {}", &script_path.display());

    let result = util::is_file_accessible(&script_path);
    if let Err(result) = result {
        error!(
            "Script file {} is not accessible: {}",
            script_path.display(),
            result
        );

        return Err(MainError::ScriptExecError {}.into());
    }

    let result = util::is_file_accessible(util::get_manifest_for(&script_path));
    if let Err(result) = result {
        error!(
            "Manifest file for script {} is not accessible: {}",
            script_path.display(),
            result
        );

        return Err(MainError::ScriptExecError {}.into());
    }

    let builder = thread::Builder::new().name(format!(
        "{}:{}",
        thread_idx,
        script_path.file_name().unwrap().to_string_lossy(),
    ));

    builder.spawn(move || -> Result<()> {
        #[cfg(feature = "profiling")]
        coz::thread_init();

        #[allow(clippy::never_loop)]
        loop {
            let result = script::run_script(script_path.clone(), profile, &lua_rx);

            match result {
                Ok(script::RunScriptResult::TerminatedGracefully) => return Ok(()),

                Ok(script::RunScriptResult::TerminatedWithErrors) => {
                    error!("Script execution failed");

                    LUA_TXS.write().get_mut(thread_idx).unwrap().is_failed = true;
                    REQUEST_FAILSAFE_MODE.store(true, Ordering::SeqCst);

                    return Err(MainError::ScriptExecError {}.into());
                }

                Err(_e) => {
                    error!("Script execution failed due to an unknown error");

                    LUA_TXS.write().get_mut(thread_idx).unwrap().is_failed = true;
                    REQUEST_FAILSAFE_MODE.store(true, Ordering::SeqCst);

                    return Err(MainError::ScriptExecError {}.into());
                }
            }
        }
    })?;

    Ok(())
}

pub fn spawn_device_io_thread(dev_io_rx: Receiver<DeviceAction>) -> Result<()> {
    let builder = thread::Builder::new().name("dev-io/all".to_owned());

    builder.spawn(move || -> Result<()> {
        #[cfg(feature = "profiling")]
        coz::thread_init();

        // stores the generation number of the frame that is currently visible on the keyboard
        let saved_frame_generation = AtomicUsize::new(0);

        // used to calculate frames per second
        let mut fps_counter: i32 = 0;
        let mut fps_timer = Instant::now();

        #[allow(clippy::never_loop)]
        loop {
            // check if we shall terminate the device I/O thread
            if QUIT.load(Ordering::SeqCst) {
                break Ok(());
            }

            match dev_io_rx.recv() {
                Ok(message) => match message {
                    DeviceAction::RenderNow  => {
                        let current_frame_generation = script::FRAME_GENERATION_COUNTER.load(Ordering::SeqCst);
                        if saved_frame_generation.load(Ordering::SeqCst) < current_frame_generation {
                            // instruct the Lua VMs to realize their color maps, but only if at least one VM
                            // submitted a new color map (performed a frame generation increment)

                            // execute render "pipeline" now...
                            let mut drop_frame = false;

                            // first, start with a clear canvas
                            script::LED_MAP.write().copy_from_slice(
                                &[RGBA {
                                    r: 0,
                                    g: 0,
                                    b: 0,
                                    a: 0,
                                }; constants::CANVAS_SIZE],
                            );

                            // instruct Lua VMs to realize their color maps,
                            // e.g. to blend their local color maps with the canvas
                            *COLOR_MAPS_READY_CONDITION.0.lock() = LUA_TXS.read().len() - FAILED_TXS.read().len();

                            for (index, lua_tx) in LUA_TXS.read().iter().enumerate() {
                                // if this tx failed previously, then skip it completely
                                if !FAILED_TXS.read().contains(&index) {
                                    // guarantee the right order of execution for the alpha blend
                                    // operations, so we have to wait for the current Lua VM to
                                    // complete its blending code, before continuing
                                    let mut pending = COLOR_MAPS_READY_CONDITION.0.lock();

                                    lua_tx
                                        .send(script::Message::RealizeColorMap)
                                        .unwrap_or_else(|e| {
                                            error!("Send error during realization of color maps: {}", e);
                                            FAILED_TXS.write().insert(index);
                                        });

                                    let result = COLOR_MAPS_READY_CONDITION.1.wait_for(
                                        &mut pending,
                                        Duration::from_millis(constants::TIMEOUT_CONDITION_MILLIS),
                                    );

                                    if result.timed_out() {
                                        drop_frame = true;
                                        warn!("Frame dropped: Timeout while waiting for a lock!");
                                        break;
                                    }
                                } else {
                                    drop_frame = true;
                                }
                            }

                            if ULEDS_SUPPORT_ACTIVE.load(Ordering::SeqCst) {
                                // blend the LED map of the Userspace LEDs support plugin
                                let uleds_led_map = uleds::LED_MAP.read();
                                let brightness = crate::BRIGHTNESS.load(Ordering::SeqCst);

                                for chunks in script::LED_MAP.write().chunks_exact_mut(constants::CANVAS_SIZE) {
                                    for (idx, background) in chunks.iter_mut().enumerate() {
                                        let bg = &background;
                                        let fg = uleds_led_map[idx];

                                        #[rustfmt::skip]
                                        let color = RGBA {
                                            r: ((((fg.a as f32) * fg.r as f32 + (255 - fg.a) as f32 * bg.r as f32).floor() * brightness as f32 / 100.0) as u32 >> 8) as u8,
                                            g: ((((fg.a as f32) * fg.g as f32 + (255 - fg.a) as f32 * bg.g as f32).floor() * brightness as f32 / 100.0) as u32 >> 8) as u8,
                                            b: ((((fg.a as f32) * fg.b as f32 + (255 - fg.a) as f32 * bg.b as f32).floor() * brightness as f32 / 100.0) as u32 >> 8) as u8,
                                            a: fg.a as u8,
                                        };

                                        *background = color;
                                    }
                                }
                            }

                            if SDK_SUPPORT_ACTIVE.load(Ordering::SeqCst) {
                                // finally, blend the LED map of the SDK support plugin
                                let sdk_led_map = sdk_support::LED_MAP.read();
                                let brightness = crate::BRIGHTNESS.load(Ordering::SeqCst);

                                for chunks in script::LED_MAP.write().chunks_exact_mut(constants::CANVAS_SIZE) {
                                    for (idx, background) in chunks.iter_mut().enumerate() {
                                        let bg = &background;
                                        let fg = sdk_led_map[idx];

                                        #[rustfmt::skip]
                                        let color = RGBA {
                                            r: ((((fg.a as f32) * fg.r as f32 + (255 - fg.a) as f32 * bg.r as f32).floor() * brightness as f32 / 100.0) as u32 >> 8) as u8,
                                            g: ((((fg.a as f32) * fg.g as f32 + (255 - fg.a) as f32 * bg.g as f32).floor() * brightness as f32 / 100.0) as u32 >> 8) as u8,
                                            b: ((((fg.a as f32) * fg.b as f32 + (255 - fg.a) as f32 * bg.b as f32).floor() * brightness as f32 / 100.0) as u32 >> 8) as u8,
                                            a: fg.a as u8,
                                        };

                                        *background = color;
                                    }
                                }
                            }

                            // number of pending blend ops should have reached zero by now
                            // may currently occur during switching of profiles
                            let ops_pending = *COLOR_MAPS_READY_CONDITION.0.lock();
                            if ops_pending > 0 {
                                debug!(
                                    "Pending blend ops before writing LED map to device: {}",
                                    ops_pending
                                        );
                            }

                            // send the final (combined) color map to all of the devices
                            if !drop_frame {
                                for keyboard_device in crate::KEYBOARD_DEVICES.read().iter() {
                                    if let Some(mut device) = keyboard_device.try_write() {
                                        if let Ok(is_initialized) = device.is_initialized() {
                                            if is_initialized {
                                                if let Err(e) = device.send_led_map(&script::LED_MAP.read()) {
                                                    error!("Error sending LED map to a device: {}", e);

                                                    if device.has_failed().unwrap_or(true) {
                                                        warn!("Trying to unplug the failed device");

                                                        // we need to terminate and then re-enter the main loop to update all global state
                                                        crate::REENTER_MAIN_LOOP.store(true, Ordering::SeqCst);
                                                    }
                                                }
                                            } else {
                                                warn!("Skipping uninitialized device, trying to re-initialize it now...");

                                                let hidapi = crate::HIDAPI.read();
                                                let hidapi = hidapi.as_ref().unwrap();

                                                device.open(hidapi).unwrap_or_else(|e| {
                                                    error!("Error opening the keyboard device: {}", e);
                                                });

                                                // send initialization handshake
                                                info!("Initializing keyboard device...");
                                                device
                                                    .send_init_sequence()
                                                    .unwrap_or_else(|e| error!("Could not initialize the device: {}", e));
                                            }
                                        } else {
                                            warn!("Could not query device status");
                                        }
                                    } else {
                                        warn!("Skipped rendering a frame to a device, because we could not acquire a lock");
                                    }
                                }

                                for mouse_device in crate::MOUSE_DEVICES.read().iter() {
                                    if let Some(mut device) = mouse_device.try_write() {
                                        if let Ok(is_initialized) = device.is_initialized() {
                                            if is_initialized {
                                                if let Err(e) = device.send_led_map(&script::LED_MAP.read()) {
                                                    error!("Error sending LED map to a device: {}", e);

                                                    if device.has_failed().unwrap_or(true) {
                                                        warn!("Trying to unplug the failed device");

                                                        // we need to terminate and then re-enter the main loop to update all global state
                                                        crate::REENTER_MAIN_LOOP.store(true, Ordering::SeqCst);
                                                    }
                                                }
                                            } else {
                                                warn!("Skipping uninitialized device, trying to re-initialize it now...");

                                                let hidapi = crate::HIDAPI.read();
                                                let hidapi = hidapi.as_ref().unwrap();

                                                device.open(hidapi).unwrap_or_else(|e| {
                                                    error!("Error opening the mouse device: {}", e);
                                                });

                                                // send initialization handshake
                                                info!("Initializing mouse device...");
                                                device
                                                    .send_init_sequence()
                                                    .unwrap_or_else(|e| error!("Could not initialize the device: {}", e));
                                            }
                                        } else {
                                            warn!("Could not query device status");
                                        }
                                    } else {
                                        warn!("Skipped rendering a frame to a device, because we could not acquire a lock");
                                    }
                                }

                                for misc_device in crate::MISC_DEVICES.read().iter() {
                                    if let Some(mut device) = misc_device.try_write() {
                                        if let Ok(is_initialized) = device.is_initialized() {
                                            if is_initialized {
                                                if let Err(e) = device.send_led_map(&script::LED_MAP.read()) {
                                                    error!("Error sending LED map to a device: {}", e);

                                                    if device.has_failed().unwrap_or(true) {
                                                        warn!("Trying to unplug the failed device");

                                                        // we need to terminate and then re-enter the main loop to update all global state
                                                        crate::REENTER_MAIN_LOOP.store(true, Ordering::SeqCst);
                                                    }
                                                }
                                            } else {
                                                warn!("Skipping uninitialized device, trying to re-initialize it now...");

                                                let hidapi = crate::HIDAPI.read();
                                                let hidapi = hidapi.as_ref().unwrap();

                                                device.open(hidapi).unwrap_or_else(|e| {
                                                    error!("Error opening the misc device: {}", e);
                                                });

                                                // send initialization handshake
                                                info!("Initializing misc device...");
                                                device
                                                    .send_init_sequence()
                                                    .unwrap_or_else(|e| error!("Could not initialize the device: {}", e));
                                            }
                                        } else {
                                            warn!("Could not query device status");
                                        }
                                    } else {
                                        warn!("Skipped rendering a frame to a device, because we could not acquire a lock");
                                    }
                                }

                                // update the current frame generation
                                saved_frame_generation.store(current_frame_generation, Ordering::SeqCst);

                                script::LAST_RENDERED_LED_MAP
                                    .write()
                                    .copy_from_slice(&script::LED_MAP.read());
                            }

                            fps_counter += 1;
                        }

                        // calculate and log fps each second
                        if fps_timer.elapsed().as_millis() >= 1000 {
                            debug!("FPS: {}", fps_counter);

                            fps_timer = Instant::now();
                            fps_counter = 0;
                        }
                    }
                },

                Err(e) => {
                    error!("Could not receive data: {}", e)
                }
            }
        }
    })?;

    Ok(())
}
