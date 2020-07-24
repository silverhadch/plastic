use super::{error::CartridgeError, mapper::Mapper, mappers::*};
use common::{
    interconnection::CartridgeCPUConnection, Bus, Device, MirroringMode, MirroringProvider,
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};

pub struct Cartridge {
    // header
    size_prg: u8,
    size_chr: u8,
    mapper_id: u8,
    mirroring_vertical: bool,
    contain_sram: bool,
    contain_trainer: bool,
    use_4_screen_mirroring: bool,
    vs_unisystem: bool,        // don't know what is this (flag 7)
    _playchoice_10_hint: bool, // not used
    is_nes_2: bool,

    pub trainer_data: Vec<u8>,
    pub prg_data: Vec<u8>,
    pub chr_data: Vec<u8>,

    mapper: Box<dyn Mapper>,
}

impl Cartridge {
    // TODO: not sure if it should consume the file or not
    pub fn from_file(mut file: File) -> Result<Self, CartridgeError> {
        let mut header = [0; 16];
        file.read_exact(&mut header)?;

        // decode header
        Cartridge::check_magic(&header[0..4])?;

        let size_prg = header[4];
        let size_chr = header[5];

        let mirroring_vertical = header[6] & 1 != 0;
        header[6] >>= 1;
        let contain_sram = header[6] & 1 != 0;
        header[6] >>= 1;
        let contain_trainer = header[6] & 1 != 0;
        header[6] >>= 1;
        let use_4_screen_mirroring = header[6] & 1 != 0;
        header[6] >>= 1;
        let lower_mapper = header[6]; // the rest

        let vs_unisystem = header[7] & 1 != 0;
        header[7] >>= 1;
        let _playchoice_10_hint = header[7] & 1 != 0;
        header[7] >>= 1;
        let is_nes_2 = (header[7] & 0b11) == 2;
        header[7] >>= 2;
        let upper_mapper = header[7]; // the rest

        let mapper_id = upper_mapper << 4 | lower_mapper;

        // initialize the mapper first, so that if it is not supported yet,
        // panic
        let mapper = Self::get_mapper(mapper_id, size_prg, size_chr);

        let mut trainer_data = Vec::new();

        // read training data if present
        if contain_trainer {
            trainer_data.resize(512, 0);
            file.read_exact(&mut trainer_data)?;
        }

        // read PRG data
        let mut prg_data = Vec::new();
        prg_data.resize((size_prg as usize) * 16 * 1024, 0);
        file.read_exact(&mut prg_data)?;

        // read CHR data
        let mut chr_data = Vec::new();
        if size_chr != 0 {
            chr_data.resize((size_chr as usize) * 8 * 1024, 0);
            file.read_exact(&mut chr_data)?;
        } else {
            // use CHR RAM
            chr_data.resize(1 * 8 * 1024, 0);
        }

        if is_nes_2 {
            // print a warning message just to know which games need INES2.
            eprintln!(
                "[WARN], the cartridge header is in INES2.0 format, but \
                this emulator only supports INES1.0, the game might work \
                but mostly it will be buggy"
            );
        }

        // there are missing parts
        let current = file.seek(SeekFrom::Current(0))?;
        let end = file.seek(SeekFrom::End(0))?;
        if current != end {
            Err(CartridgeError::TooLargeFile(end - current))
        } else {
            Ok(Self {
                size_prg,
                size_chr,
                mapper_id,
                mirroring_vertical,
                contain_sram,
                contain_trainer,
                use_4_screen_mirroring,
                vs_unisystem,
                _playchoice_10_hint,
                is_nes_2,
                trainer_data,
                prg_data,
                chr_data,
                mapper,
            })
        }
    }

    pub fn is_vertical_mirroring(&self) -> bool {
        self.mirroring_vertical
    }

    fn check_magic(header: &[u8]) -> Result<(), CartridgeError> {
        let real = [0x4E, 0x45, 0x53, 0x1A];

        if header == real {
            Ok(())
        } else {
            Err(CartridgeError::HeaderError)
        }
    }

    fn get_mapper(mapper_id: u8, prg_count: u8, chr_count: u8) -> Box<dyn Mapper> {
        let mut mapper: Box<dyn Mapper> = match mapper_id {
            0 => Box::new(Mapper0::new()),
            1 => Box::new(Mapper1::new()),
            2 => Box::new(Mapper2::new()),
            3 => Box::new(Mapper3::new()),
            4 => Box::new(Mapper4::new()),
            _ => {
                unimplemented!("Mapper {} is not yet implemented", mapper_id);
            }
        };

        // should always call init in a new mapper, as it is the only way
        // they share a constructor
        mapper.init(prg_count, chr_count);

        mapper
    }

    fn is_chr_ram(&self) -> bool {
        self.size_chr == 0
    }
}

impl Bus for Cartridge {
    fn read(&self, address: u16, device: Device) -> u8 {
        let address = self.mapper.map_read(address, device);

        match device {
            // CPU is reading PRG only
            Device::CPU => *self.prg_data.get(address).expect("PRG out of bounds"),
            // PPU is reading CHR data
            Device::PPU => *self.chr_data.get(address).expect("CHR out of bounds"),
        }
    }
    fn write(&mut self, address: u16, data: u8, device: Device) {
        // send the write signal, this might trigger bank change
        self.mapper.map_write(address, data, device);

        match device {
            Device::CPU => {
                // ## This is only a ROM data (read only) ##
                // *self
                //     .prg_data
                //     .get_mut(address as usize)
                //     .expect("PRG out of bounds") = data;
            }
            Device::PPU => {
                if self.is_chr_ram() {
                    *self
                        .chr_data
                        .get_mut(address as usize)
                        .expect("CHR out of bounds") = data;
                }
            }
        }
    }
}

impl MirroringProvider for Cartridge {
    fn mirroring_mode(&self) -> MirroringMode {
        if self.use_4_screen_mirroring {
            MirroringMode::FourScreen
        } else {
            if self.mapper.is_hardwired_mirrored() {
                if self.mirroring_vertical {
                    MirroringMode::Vertical
                } else {
                    MirroringMode::Horizontal
                }
            } else {
                self.mapper.nametable_mirroring()
            }
        }
    }
}

impl CartridgeCPUConnection for Cartridge {
    fn is_irq_change_requested(&self) -> bool {
        self.mapper.is_irq_pin_state_changed_requested()
    }

    fn irq_pin_state(&self) -> bool {
        self.mapper.irq_pin_state()
    }

    fn clear_irq_request_pin(&mut self) {
        self.mapper.clear_irq_request_pin();
    }
}
