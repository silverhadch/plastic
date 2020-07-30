#[cfg(test)]
mod cpu_tests {
    use nes_tester::{TestError, NES};

    use crate::{CPURunState, CPU6502};
    use common::{interconnection::*, Bus, Device};
    use std::{cell::RefCell, rc::Rc};

    struct DummyBus {
        data: [u8; 0x10000],
    }

    impl DummyBus {
        pub fn new(data: [u8; 0x10000]) -> Self {
            Self { data }
        }
    }

    impl Bus for DummyBus {
        fn read(&self, address: u16, _: Device) -> u8 {
            self.data[address as usize]
        }
        fn write(&mut self, address: u16, data: u8, _: Device) {
            self.data[address as usize] = data;
        }
    }

    struct DummyCartridgePPUHandler {}

    impl PPUCPUConnection for DummyCartridgePPUHandler {
        fn is_nmi_pin_set(&self) -> bool {
            false
        }
        fn clear_nmi_pin(&mut self) {}
        fn is_dma_request(&self) -> bool {
            false
        }
        fn clear_dma_request(&mut self) {}
        fn dma_address(&mut self) -> u8 {
            unreachable!()
        }
        fn send_oam_data(&mut self, _address: u8, _data: u8) {
            unreachable!();
        }
    }

    impl CartridgeCPUConnection for DummyCartridgePPUHandler {
        fn is_irq_change_requested(&self) -> bool {
            false
        }

        fn irq_pin_state(&self) -> bool {
            unreachable!()
        }

        fn clear_irq_request_pin(&mut self) {}
    }

    fn run_blargg_test(filename: &str) -> Result<(), TestError> {
        let mut nes = NES::new(filename)?;
        nes.reset_cpu();

        let result_location = 0x6000;

        nes.clock_until_infinite_loop();
        nes.clock_until_memory_neq(result_location, 0x80);

        let result = nes.cpu_read_address(result_location);

        if result != 0 {
            Err(TestError::ResultError(result))
        } else {
            Ok(())
        }
    }

    #[test]
    fn functionality_test() {
        let file_data =
            include_bytes!("../tests/roms/6502_functional_test/6502_functional_test.bin");
        let mut data = [0; 0x10000];
        data[0xa..file_data.len() + 0xa].clone_from_slice(file_data);

        // set the reset vector pointer to 0x0400
        data[0xFFFC] = 0x00;
        data[0xFFFD] = 0x04;

        const SUCCUSS_ADDRESS: u16 = 0x336D;

        let bus = DummyBus::new(data);
        let handler = Rc::new(RefCell::new(DummyCartridgePPUHandler {}));
        let mut cpu = CPU6502::new(Rc::new(RefCell::new(bus)), handler.clone(), handler);

        cpu.reset();

        loop {
            let state = cpu.run_next();

            // if we stuck in a loop, return error
            if let CPURunState::InfiniteLoop(pc) = state {
                assert!(
                    pc == SUCCUSS_ADDRESS,
                    "Test failed at {:04X}, check the `.lst` file for more info",
                    pc
                );
                break;
            }
        }
    }

    #[test]
    fn instructions_test() -> Result<(), TestError> {
        run_blargg_test("./tests/roms/instr_test-v5/all_instrs.nes")
    }

    #[test]
    fn instructions_timing_test() -> Result<(), TestError> {
        run_blargg_test("./tests/roms/instr_timing/instr_timing.nes")
    }
}