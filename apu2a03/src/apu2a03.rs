use crate::apu2a03_registers::Register;
use crate::channels::{Dmc, NoiseWave, SquarePulse, TriangleWave};
use crate::envelope::EnvelopedChannel;
use crate::length_counter::LengthCountedChannel;
use crate::tone_source::{APUChannel, APUChannelPlayer, BufferedChannel, TimedAPUChannel};
use common::interconnection::{APUCPUConnection, CpuIrqProvider};
use std::cell::Cell;
use std::sync::{Arc, Mutex};

use rodio::DeviceTrait;

pub struct APU2A03 {
    square_pulse_1: LengthCountedChannel<SquarePulse>,
    square_pulse_2: LengthCountedChannel<SquarePulse>,
    triangle: LengthCountedChannel<TriangleWave>,
    noise: LengthCountedChannel<NoiseWave>,
    dmc: Dmc,

    buffered_channel: Arc<Mutex<BufferedChannel>>,

    is_4_step_squence_mode: bool,
    interrupt_inhibit_flag: bool,

    cycle: u16,

    wait_reset: i8,

    apu_freq: f64,
    sample_counter: f64,

    offset: f64,

    interrupt_flag: Cell<bool>,
    request_interrupt_flag_change: Cell<bool>,

    player: Option<rodio::Sink>,
}

impl APU2A03 {
    pub fn new() -> Self {
        let buffered_channel = Arc::new(Mutex::new(BufferedChannel::new()));

        Self {
            square_pulse_1: LengthCountedChannel::new(SquarePulse::new(true)),
            square_pulse_2: LengthCountedChannel::new(SquarePulse::new(false)),

            triangle: LengthCountedChannel::new(TriangleWave::new()),

            noise: LengthCountedChannel::new(NoiseWave::new()),

            dmc: Dmc::new(),

            buffered_channel: buffered_channel.clone(),

            is_4_step_squence_mode: false,
            interrupt_inhibit_flag: false,

            cycle: 0,

            apu_freq: 0.,
            sample_counter: 0.,

            offset: 0.5,

            wait_reset: 0,

            interrupt_flag: Cell::new(false),
            request_interrupt_flag_change: Cell::new(false),

            player: Self::get_player(buffered_channel.clone()),
        }
    }

    fn get_player<S: APUChannel + Send + 'static>(channel: Arc<Mutex<S>>) -> Option<rodio::Sink> {
        let device = rodio::default_output_device()?;

        // bug in rodio, that it panics if the device does not support any format
        // it is fixed now in github, not sure when is the release coming
        let formats = device.supported_output_formats().ok()?;
        if formats.count() > 0 {
            let sink = rodio::Sink::new(&device);

            let low_pass_player = rodio::source::Source::low_pass(
                APUChannelPlayer::from_clone(channel.clone()),
                10000,
            );

            sink.append(low_pass_player);
            sink.set_volume(0.15);

            sink.pause();

            Some(sink)
        } else {
            None
        }
    }

    pub(crate) fn read_register(&self, register: Register) -> u8 {
        match register {
            Register::Status => {
                let sqr1_length_counter =
                    (self.square_pulse_1.length_counter().counter() != 0) as u8;

                let sqr2_length_counter =
                    (self.square_pulse_2.length_counter().counter() != 0) as u8;

                let triangle_length_counter = (self.triangle.length_counter().counter() != 0) as u8;

                let noise_length_counter = (self.noise.length_counter().counter() != 0) as u8;

                let dmc_active = self.dmc.sample_remaining_bytes_more_than_0() as u8;
                let dmc_interrupt = self.dmc.get_irq_pin_state() as u8;

                let frame_interrupt = self.interrupt_flag.get() as u8;
                self.interrupt_flag.set(false);
                self.request_interrupt_flag_change.set(true);

                dmc_interrupt << 7
                    | frame_interrupt << 6
                    | dmc_active << 4
                    | noise_length_counter << 3
                    | triangle_length_counter << 2
                    | sqr2_length_counter << 1
                    | sqr1_length_counter
            }
            _ => {
                // unreadable
                0
            }
        }
    }

    pub(crate) fn write_register(&mut self, register: Register, data: u8) {
        match register {
            Register::Pulse1_1 => {
                let duty_cycle_index = data >> 6;
                let volume = data & 0xF;
                let use_volume = data & 0x10 != 0;
                let halt = data & 0x20 != 0;

                self.square_pulse_1
                    .channel_mut()
                    .set_duty_cycle_index(duty_cycle_index);
                self.square_pulse_1
                    .channel_mut()
                    .envelope_generator_mut()
                    .set_volume(volume, use_volume);

                self.square_pulse_1.length_counter_mut().set_halt(halt);
                self.square_pulse_1
                    .channel_mut()
                    .envelope_generator_mut()
                    .set_loop_flag(halt);

                self.square_pulse_1
                    .channel_mut()
                    .envelope_generator_mut()
                    .set_start_flag(true);
            }
            Register::Pulse1_2 => {
                // sweep
                self.square_pulse_1.channel_mut().set_sweeper_data(data);
            }
            Register::Pulse1_3 => {
                let period = self.square_pulse_1.channel().get_period();

                // lower timer bits
                self.square_pulse_1
                    .channel_mut()
                    .set_period((period & 0xFF00) | data as u16);
            }
            Register::Pulse1_4 => {
                self.square_pulse_1
                    .length_counter_mut()
                    .reload_counter(data >> 3);

                let period = self.square_pulse_1.channel().get_period();

                // high timer bits
                self.square_pulse_1
                    .channel_mut()
                    .set_period((period & 0xFF) | ((data as u16 & 0b111) << 8));

                self.square_pulse_1
                    .channel_mut()
                    .envelope_generator_mut()
                    .set_start_flag(true);

                // reset pulse
                self.square_pulse_1.channel_mut().reset();
            }
            Register::Pulse2_1 => {
                let duty_cycle_index = data >> 6;
                let volume = data & 0xF;
                let use_volume = data & 0x10 != 0;
                let halt = data & 0x20 != 0;

                self.square_pulse_2
                    .channel_mut()
                    .set_duty_cycle_index(duty_cycle_index);
                self.square_pulse_2
                    .channel_mut()
                    .envelope_generator_mut()
                    .set_volume(volume, use_volume);

                self.square_pulse_2.length_counter_mut().set_halt(halt);
                self.square_pulse_2
                    .channel_mut()
                    .envelope_generator_mut()
                    .set_loop_flag(halt);
                self.square_pulse_2
                    .channel_mut()
                    .envelope_generator_mut()
                    .set_start_flag(true);
            }
            Register::Pulse2_2 => {
                // sweep
                self.square_pulse_2.channel_mut().set_sweeper_data(data);
            }
            Register::Pulse2_3 => {
                let period = self.square_pulse_2.channel().get_period();

                // lower timer bits
                self.square_pulse_2
                    .channel_mut()
                    .set_period((period & 0xFF00) | data as u16);
            }
            Register::Pulse2_4 => {
                self.square_pulse_2
                    .length_counter_mut()
                    .reload_counter(data >> 3);

                let period = self.square_pulse_2.channel().get_period();

                // high timer bits
                self.square_pulse_2
                    .channel_mut()
                    .set_period((period & 0xFF) | ((data as u16 & 0b111) << 8));

                self.square_pulse_2
                    .channel_mut()
                    .envelope_generator_mut()
                    .set_start_flag(true);

                // reset pulse
                self.square_pulse_2.channel_mut().reset();
            }
            Register::Triangle1 => {
                self.triangle
                    .channel_mut()
                    .set_linear_counter_reload_value(data & 0x7F);
                self.triangle
                    .channel_mut()
                    .set_linear_counter_control_flag(data & 0x80 != 0);

                self.triangle
                    .length_counter_mut()
                    .set_halt(data & 0x80 != 0);
            }
            Register::Triangle2 => {
                // unused
            }
            Register::Triangle3 => {
                let period = self.triangle.channel().get_period();

                // lower timer bits
                self.triangle
                    .channel_mut()
                    .set_period((period & 0xFF00) | data as u16);
            }
            Register::Triangle4 => {
                self.triangle.length_counter_mut().reload_counter(data >> 3);

                let period = self.triangle.channel().get_period();

                // high timer bits
                self.triangle
                    .channel_mut()
                    .set_period((period & 0xFF) | ((data as u16 & 0b111) << 8));

                self.triangle
                    .channel_mut()
                    .set_linear_counter_reload_flag(true);
            }
            Register::Noise1 => {
                let volume = data & 0xF;
                let use_volume = data & 0x10 != 0;
                let halt = data & 0x20 != 0;

                self.noise
                    .channel_mut()
                    .envelope_generator_mut()
                    .set_volume(volume, use_volume);
                self.noise.length_counter_mut().set_halt(halt);
                self.noise
                    .channel_mut()
                    .envelope_generator_mut()
                    .set_loop_flag(halt);
                self.noise
                    .channel_mut()
                    .envelope_generator_mut()
                    .set_start_flag(true);
            }
            Register::Noise2 => {
                // unused
            }
            Register::Noise3 => {
                self.noise.channel_mut().set_mode_flag(data & 0x80 != 0);
                self.noise.channel_mut().set_period(data & 0xF);
            }
            Register::Noise4 => {
                self.noise.length_counter_mut().reload_counter(data >> 3);
            }
            Register::DMC1 => {
                let rate_index = data & 0xF;
                let loop_flag = data & 0x40 != 0;
                let irq_enabled = data & 0x80 != 0;

                self.dmc.set_rate_index(rate_index);
                self.dmc.set_loop_flag(loop_flag);
                self.dmc.set_irq_enabled_flag(irq_enabled);
            }
            Register::DMC2 => {
                self.dmc.set_direct_output_level_load(data & 0x7F);
            }
            Register::DMC3 => {
                self.dmc.set_samples_address(data);
            }
            Register::DMC4 => {
                self.dmc.set_samples_length(data);
            }
            Register::Status => {
                // enable and disable length counters
                self.square_pulse_1
                    .length_counter_mut()
                    .set_enabled((data >> 0 & 1) != 0);

                self.square_pulse_2
                    .length_counter_mut()
                    .set_enabled((data >> 1 & 1) != 0);

                self.triangle
                    .length_counter_mut()
                    .set_enabled((data >> 2 & 1) != 0);

                self.noise
                    .length_counter_mut()
                    .set_enabled((data >> 3 & 1) != 0);

                if data >> 4 & 1 == 0 {
                    self.dmc.clear_sample_remaining_bytes_and_silence();
                } else {
                    if !self.dmc.sample_remaining_bytes_more_than_0() {
                        self.dmc.restart_sample();
                    }
                }

                self.dmc.clear_interrupt_flag();
            }
            Register::FrameCounter => {
                self.is_4_step_squence_mode = data & 0x80 == 0;
                self.interrupt_inhibit_flag = data & 0x40 != 0;

                if self.interrupt_inhibit_flag {
                    self.interrupt_flag.set(false);
                    self.request_interrupt_flag_change.set(true);
                }

                // clock immediately
                if data & 0x80 != 0 {
                    self.generate_half_frame_clock();
                    self.generate_quarter_frame_clock();
                } else {
                    // reset(side effect)
                    self.wait_reset = 2; // after 4 CPU clocks
                }
            }
        }
    }

    pub fn play(&self) {
        if let Some(ref player) = self.player {
            player.play();
        }
    }

    pub fn pause(&self) {
        if let Some(ref player) = self.player {
            player.pause();
        }
    }

    fn generate_quarter_frame_clock(&mut self) {
        self.square_pulse_1.clock_envlope();
        self.square_pulse_2.clock_envlope();
        self.noise.clock_envlope();
        self.triangle.channel_mut().clock_linear_counter();
    }

    fn generate_half_frame_clock(&mut self) {
        self.square_pulse_1.length_counter_mut().decrement();
        self.square_pulse_1.channel_mut().clock_sweeper();
        self.square_pulse_2.length_counter_mut().decrement();
        self.square_pulse_2.channel_mut().clock_sweeper();
        self.triangle.length_counter_mut().decrement();
        self.noise.length_counter_mut().decrement();
    }

    pub fn update_apu_freq(&mut self, apu_freq: f64) {
        self.apu_freq = apu_freq;
    }

    fn get_mixer_output(&mut self) -> f32 {
        let square_pulse_1 = self.square_pulse_1.get_output();
        let square_pulse_2 = self.square_pulse_2.get_output();
        let triangle = self.triangle.get_output();
        let noise = self.noise.get_output();
        let dmc = self.dmc.get_output();

        let pulse_out = if square_pulse_1 == 0. && square_pulse_2 == 0. {
            0.
        } else {
            95.88 / ((8128. / (square_pulse_1 + square_pulse_2)) + 100.)
        };

        let tnd_out = if triangle == 0. && noise == 0. && dmc == 0. {
            0.
        } else {
            159.79 / ((1. / ((triangle / 8227.) + (noise / 12241.) + (dmc / 22638.))) + 100.)
        };

        pulse_out + tnd_out
    }

    pub fn clock(&mut self) {
        if self.wait_reset > 0 {
            self.wait_reset -= 1;
        } else if self.wait_reset == 0 {
            self.cycle = 0;
            self.wait_reset = -1;

            // mode bit is set
            if !self.is_4_step_squence_mode {
                self.generate_quarter_frame_clock();
                self.generate_half_frame_clock();
            }
        }

        // after how many apu clocks a sample should be recorded
        let samples_every_n_apu_clock = (self.apu_freq / (crate::SAMPLE_RATE as f64)) - self.offset;

        if self.cycle % 300 == 0 {
            if let Ok(mut buffered_channel) = self.buffered_channel.lock() {
                let change = if buffered_channel.get_is_overusing() {
                    0.001
                } else if buffered_channel.get_is_underusing() {
                    -0.0002
                } else {
                    0.
                };

                self.offset += change;
                buffered_channel.clear_using_flags();
            }
        }

        self.sample_counter += 1.0;
        if self.sample_counter >= samples_every_n_apu_clock {
            let output = self.get_mixer_output();

            self.buffered_channel.lock().unwrap().recored_sample(output);

            self.sample_counter -= samples_every_n_apu_clock;
        }

        self.square_pulse_1.timer_clock();
        self.square_pulse_2.timer_clock();
        self.triangle.timer_clock();
        self.triangle.timer_clock();
        self.noise.timer_clock();
        self.dmc.timer_clock();

        self.cycle += 1;

        match self.cycle {
            3729 => {
                self.generate_quarter_frame_clock();
            }
            7457 => {
                self.generate_quarter_frame_clock();
                self.generate_half_frame_clock();
            }
            11186 => {
                self.generate_quarter_frame_clock();
            }
            14915 if self.is_4_step_squence_mode => {
                self.generate_quarter_frame_clock();
                self.generate_half_frame_clock();

                if !self.interrupt_inhibit_flag {
                    self.interrupt_flag.set(true);
                    self.request_interrupt_flag_change.set(true);
                }
                self.cycle = 0;
            }
            18641 if !self.is_4_step_squence_mode => {
                self.generate_quarter_frame_clock();
                self.generate_half_frame_clock();
                self.cycle = 0;
            }
            _ => {
                // ignore
            }
        }
    }
}

impl CpuIrqProvider for APU2A03 {
    fn is_irq_change_requested(&self) -> bool {
        let dmc_irq_request = self.dmc.is_irq_change_requested();

        self.request_interrupt_flag_change.get() || dmc_irq_request
    }

    fn irq_pin_state(&self) -> bool {
        let dmc_irq = self.dmc.get_irq_pin_state();

        self.interrupt_flag.get() || dmc_irq
    }

    fn clear_irq_request_pin(&mut self) {
        self.request_interrupt_flag_change.set(false);

        self.dmc.clear_irq_request_pin();
    }
}

impl APUCPUConnection for APU2A03 {
    fn request_dmc_reader_read(&self) -> Option<u16> {
        self.dmc.request_dmc_reader_read()
    }

    fn submit_buffer_byte(&mut self, byte: u8) {
        self.dmc.submit_buffer_byte(byte);
    }
}
