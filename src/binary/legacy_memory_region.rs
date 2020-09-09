use crate::memory_map::MemoryRegion;
use core::mem::MaybeUninit;
use x86_64::{
    structures::paging::{FrameAllocator, PhysFrame, Size4KiB},
    PhysAddr,
};

pub trait LegacyMemoryRegion: Copy + core::fmt::Debug {
    fn start(&self) -> PhysAddr;
    fn len(&self) -> u64;
    fn usable(&self) -> bool;

    fn set_start(&mut self, new_start: PhysAddr);
}

pub struct LegacyFrameAllocator<I, D> {
    original: I,
    memory_map: I,
    current_descriptor: Option<D>,
    next_frame: PhysFrame,
}

impl<I, D> LegacyFrameAllocator<I, D>
where
    I: ExactSizeIterator<Item = D> + Clone,
    I::Item: LegacyMemoryRegion,
{
    pub fn new(memory_map: I) -> Self {
        // skip frame 0 because the rust core library does not see 0 as a valid address
        let start_frame = PhysFrame::containing_address(PhysAddr::new(0x1000));
        Self::new_starting_at(start_frame, memory_map)
    }

    pub fn new_starting_at(frame: PhysFrame, memory_map: I) -> Self {
        Self {
            original: memory_map.clone(),
            memory_map,
            current_descriptor: None,
            next_frame: frame,
        }
    }

    fn allocate_frame_from_descriptor(&mut self, descriptor: D) -> Option<PhysFrame> {
        let start_addr = descriptor.start();
        let start_frame = PhysFrame::containing_address(start_addr);
        let end_addr = start_addr + descriptor.len();
        let end_frame = PhysFrame::containing_address(end_addr - 1u64);

        // increase self.next_frame to start_frame if smaller
        if self.next_frame < start_frame {
            self.next_frame = start_frame;
        }

        if self.next_frame < end_frame {
            let ret = self.next_frame;
            self.next_frame += 1;
            Some(ret)
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.original.len()
    }

    pub fn max_phys_addr(&self) -> PhysAddr {
        self.original
            .clone()
            .map(|r| r.start() + r.len())
            .max()
            .unwrap()
    }

    pub fn construct_memory_map(
        self,
        regions: &mut [MaybeUninit<MemoryRegion>],
    ) -> &mut [MemoryRegion] {
        use crate::memory_map::MemoryRegionKind;

        let mut next_index = 0;

        for mut descriptor in self.original {
            let end = descriptor.start() + descriptor.len();
            let next_free = self.next_frame.start_address();
            let kind = if descriptor.usable() {
                if end <= next_free {
                    MemoryRegionKind::Bootloader
                } else if descriptor.start() >= next_free {
                    MemoryRegionKind::Usable
                } else {
                    // part of the region is used -> add is separately
                    let used_region = MemoryRegion {
                        start: descriptor.start().as_u64(),
                        end: next_free.as_u64(),
                        kind: MemoryRegionKind::Bootloader,
                    };
                    Self::add_region(used_region, regions, &mut next_index)
                        .expect("Failed to add memory region");

                    // add unused part normally
                    descriptor.set_start(next_free);
                    MemoryRegionKind::Usable
                }
            } else {
                MemoryRegionKind::Reserved // FIXME more types
            };

            let region = MemoryRegion {
                start: descriptor.start().as_u64(),
                end: end.as_u64(),
                kind,
            };
            Self::add_region(region, regions, &mut next_index).unwrap();
        }

        let initialized = &mut regions[..next_index];
        unsafe { MaybeUninit::slice_assume_init_mut(initialized) }
    }

    fn add_region(
        region: MemoryRegion,
        regions: &mut [MaybeUninit<MemoryRegion>],
        next_index: &mut usize,
    ) -> Result<(), ()> {
        unsafe {
            regions
                .get_mut(*next_index)
                .ok_or(())?
                .as_mut_ptr()
                .write(region)
        };
        *next_index += 1;
        Ok(())
    }
}

unsafe impl<I, D> FrameAllocator<Size4KiB> for LegacyFrameAllocator<I, D>
where
    I: ExactSizeIterator<Item = D> + Clone,
    I::Item: LegacyMemoryRegion,
{
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        if let Some(current_descriptor) = self.current_descriptor {
            match self.allocate_frame_from_descriptor(current_descriptor) {
                Some(frame) => return Some(frame),
                None => {
                    self.current_descriptor = None;
                }
            }
        }

        // find next suitable descriptor
        while let Some(descriptor) = self.memory_map.next() {
            if !descriptor.usable() {
                continue;
            }
            if let Some(frame) = self.allocate_frame_from_descriptor(descriptor) {
                self.current_descriptor = Some(descriptor);
                return Some(frame);
            }
        }

        None
    }
}
