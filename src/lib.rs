extern crate alloc;

use crate::ffi::{DyHook, SFHook};
use anyhow::anyhow;
use log::{debug, error, warn};
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::marker::PhantomData;
use std::process::exit;
use std::ptr::null_mut;
use std::sync::{Arc, Mutex, OnceLock};

mod ffi;

pub fn guarded_fast_paths_enabled() -> bool {
    unsafe { ffi::dynarmic_guarded_fast_paths_enabled() }
}

pub fn code_page_cache_enabled() -> bool {
    unsafe { ffi::dynarmic_code_page_cache_enabled() }
}

pub fn unsafe_fastmem_enabled() -> bool {
    unsafe { ffi::dynarmic_unsafe_fastmem_enabled() }
}

#[cfg(test)]
fn parse_default_on_flag(value: Option<&str>) -> bool {
    !value.is_some_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        )
    })
}

pub fn shared_fastmem_page_table_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("NEXIUM_DYNARMIC_SHARED_PAGE_TABLE")
            .ok()
            .is_some_and(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "on" | "yes"
                )
            })
    })
}

#[derive(Debug, PartialEq, Eq)]
enum FastmemPageTableRoute {
    InitializedShared,
    ReusedShared,
    PrivateForDifferentBase,
}

#[derive(Default)]
struct FastmemPageTableSlot {
    base: Option<usize>,
    table: usize,
}

impl FastmemPageTableSlot {
    fn get_or_allocate(
        &mut self,
        base: usize,
        allocate: impl FnOnce() -> usize,
    ) -> (usize, FastmemPageTableRoute) {
        match self.base {
            Some(shared_base) if shared_base == base => {
                (self.table, FastmemPageTableRoute::ReusedShared)
            }
            Some(_) => (allocate(), FastmemPageTableRoute::PrivateForDifferentBase),
            None => {
                let table = allocate();
                if table != 0 {
                    self.base = Some(base);
                    self.table = table;
                }
                (table, FastmemPageTableRoute::InitializedShared)
            }
        }
    }
}

fn page_table_for_fastmem(fastmem_base: *mut c_void) -> *mut *mut c_void {
    use std::sync::Once;
    static MODE_LOG: Once = Once::new();
    if !shared_fastmem_page_table_enabled() {
        MODE_LOG.call_once(|| {
            log::info!(
                "[Dynarmic] Fastmem page-table mode: private (NEXIUM_DYNARMIC_SHARED_PAGE_TABLE=0)"
            );
        });
        return unsafe { ffi::dynarmic_init_page_table() };
    }
    MODE_LOG.call_once(|| {
        log::info!("[Dynarmic] Fastmem page-table mode: process-shared by arena base");
    });

    static SLOT: OnceLock<Mutex<FastmemPageTableSlot>> = OnceLock::new();
    let mut slot = SLOT
        .get_or_init(|| Mutex::new(FastmemPageTableSlot::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (table, route) = slot.get_or_allocate(fastmem_base as usize, || unsafe {
        ffi::dynarmic_init_page_table() as usize
    });

    match route {
        FastmemPageTableRoute::InitializedShared => debug!(
            "[Dynarmic] Initialized shared fastmem page table for base={:X}",
            fastmem_base as usize
        ),
        FastmemPageTableRoute::ReusedShared => debug!(
            "[Dynarmic] Reusing shared fastmem page table for base={:X}",
            fastmem_base as usize
        ),
        FastmemPageTableRoute::PrivateForDifferentBase => warn!(
            "[Dynarmic] Fastmem base {:X} differs from the process-shared arena; using a private page table",
            fastmem_base as usize
        ),
    }
    if table == 0 {
        error!("[Dynarmic] Failed to initialize fastmem page table");
    }
    table as *mut *mut c_void
}

const SHARED_MONITOR_PROCS: u32 = 8;

fn shared_exclusive_monitor() -> *mut c_void {
    use std::sync::atomic::{AtomicPtr, Ordering};
    static MONITOR: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
    let existing = MONITOR.load(Ordering::Acquire);
    if !existing.is_null() {
        return existing;
    }
    let created = unsafe { ffi::dynarmic_init_monitor(SHARED_MONITOR_PROCS) };
    match MONITOR.compare_exchange(null_mut(), created, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => created,
        Err(winner) => winner,
    }
}

fn next_processor_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
        .min(SHARED_MONITOR_PROCS - 1)
}

pub type DynarmicContext = Arc<DynarmicContextInner>;

#[derive(Clone)]
pub struct DynarmicContextInner {
    inner_context: *mut c_void,
}

impl DynarmicContextInner {
    pub fn destroy(&self) {
        unsafe {
            ffi::dynarmic_context_free(self.inner_context);
        }
    }
}

impl Drop for DynarmicContextInner {
    fn drop(&mut self) {
        self.destroy();
    }
}

unsafe impl Send for DynarmicContextInner {}
unsafe impl Sync for DynarmicContextInner {}

pub type DynarmicContext32 = Arc<DynarmicContext32Inner>;

#[derive(Clone)]
pub struct DynarmicContext32Inner {
    inner_context: *mut c_void,
}

impl DynarmicContext32Inner {
    pub fn destroy(&self) {
        unsafe {
            ffi::dynarmic_context32_free(self.inner_context);
        }
    }
}

impl Drop for DynarmicContext32Inner {
    fn drop(&mut self) {
        self.destroy();
    }
}

unsafe impl Send for DynarmicContext32Inner {}
unsafe impl Sync for DynarmicContext32Inner {}

/// Returns the version of the underlying Dynarmic engine.
pub fn dynarmic_version() -> u32 {
    unsafe { ffi::dynarmic_version() }
}

/// A little surprise from the developer.
pub fn dynarmic_colorful_egg() -> String {
    unsafe {
        let c_str = ffi::dynarmic_colorful_egg();
        if c_str.is_null() {
            return String::new();
        }
        let c_str = std::ffi::CStr::from_ptr(c_str);
        c_str.to_string_lossy().into_owned()
    }
}

struct Metadata<'a> {
    svc_callback: Option<Box<dyn SFHook + 'a>>,
    unmapped_mem_callback: Option<Box<dyn SFHook + 'a>>,
    until: u64,
    _memory: *mut c_void,
    _monitor: *mut c_void,
    _page_table: *mut *mut c_void,
    handle: *mut c_void,
}

impl Drop for Metadata<'_> {
    fn drop(&mut self) {
        log::info!("[dynarmic] Dropping Metadata");
        unsafe {
            ffi::dynarmic_destroy(self.handle);
        }
    }
}

unsafe impl Send for Metadata<'_> {}
unsafe impl Sync for Metadata<'_> {}

/// A high-level wrapper around the Dynarmic ARM dynamic recompiler.
///
/// This struct provides a safe(r) interface for memory mapping, register access,
/// and execution control for both ARM32 and ARM64 architectures.
#[derive(Clone)]
pub struct Dynarmic<'a, T: Clone + Send + Sync> {
    cur_handle: *mut c_void,
    metadata: Arc<UnsafeCell<Metadata<'a>>>,
    pd: PhantomData<&'a T>,
}

unsafe impl<'a, T: Clone + Send + Sync> Send for Dynarmic<'a, T> {}
unsafe impl<'a, T: Clone + Send + Sync> Sync for Dynarmic<'a, T> {}

impl<'a, T: Clone + Send + Sync> Dynarmic<'a, T> {
    /// Creates a new Dynarmic instance for ARM64 (AArch64) emulation.
    ///
    /// The JIT cache size can be configured via the `DYNARMIC_JIT_SIZE` environment variable (in MB).
    /// Defaults to 64MB. Valid range: 8MB to 512MB.
    pub fn new() -> Dynarmic<'static, T> {
        let memory = unsafe { ffi::dynarmic_init_memory() };
        if memory == null_mut() {
            error!("Failed to initialize memory");
            exit(0)
        }

        let mut jit_size = std::env::var("DYNARMIC_JIT_SIZE")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(64);

        if jit_size < 8 {
            warn!("JIT size {}MB is too small, setting to 8MB", jit_size);
            jit_size = 8;
        } else if jit_size > 512 {
            warn!("JIT size {}MB is too large, setting to 512MB", jit_size);
            jit_size = 512;
        }

        let monitor = shared_exclusive_monitor();
        let processor_id = next_processor_id();
        let page_table = unsafe { ffi::dynarmic_init_page_table() };
        let handle = unsafe {
            ffi::dynarmic_new(
                processor_id,
                memory,
                monitor,
                page_table,
                jit_size * 1024 * 1024,
                true,
            )
        };

        debug!(
            "[Dynarmic] Created new Dynarmic instance: {:X} with {}MB JIT",
            handle as usize, jit_size
        );

        Dynarmic {
            cur_handle: handle,
            metadata: Arc::new(UnsafeCell::new(Metadata {
                svc_callback: None,
                unmapped_mem_callback: None,
                until: 0,
                _memory: memory,
                _monitor: monitor,
                _page_table: page_table,
                handle,
            })),
            pd: PhantomData,
        }
    }

    /// Creates a new Dynarmic instance for ARM64 (A64) emulation with fastmem.
    ///
    /// `fastmem_base` must point to a host virtual-address reservation that backs the
    /// entire guest address space (size = 1 << PAGE_TABLE_ADDRESS_SPACE_BITS), with
    /// guest pages committed at `fastmem_base + guest_va`. Guest loads/stores compile
    /// to direct host accesses; faults fall back to the memory callbacks.
    pub fn new_fastmem(fastmem_base: *mut std::ffi::c_void) -> Dynarmic<'static, T> {
        let memory = unsafe { ffi::dynarmic_init_memory() };
        if memory == null_mut() {
            error!("Failed to initialize memory");
            exit(0)
        }

        let mut jit_size = std::env::var("DYNARMIC_JIT_SIZE")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(64);

        if jit_size < 8 {
            warn!("JIT size {}MB is too small, setting to 8MB", jit_size);
            jit_size = 8;
        } else if jit_size > 512 {
            warn!("JIT size {}MB is too large, setting to 512MB", jit_size);
            jit_size = 512;
        }

        let monitor = shared_exclusive_monitor();
        let processor_id = next_processor_id();
        let page_table = page_table_for_fastmem(fastmem_base);
        let handle = unsafe {
            ffi::dynarmic_new_fm(
                processor_id,
                memory,
                monitor,
                page_table,
                jit_size * 1024 * 1024,
                true,
                fastmem_base,
            )
        };

        debug!(
            "[Dynarmic] Created fastmem Dynarmic instance: {:X} base={:X} with {}MB JIT",
            handle as usize, fastmem_base as usize, jit_size
        );

        Dynarmic {
            cur_handle: handle,
            metadata: Arc::new(UnsafeCell::new(Metadata {
                svc_callback: None,
                unmapped_mem_callback: None,
                until: 0,
                _memory: memory,
                _monitor: monitor,
                _page_table: page_table,
                handle,
            })),
            pd: PhantomData,
        }
    }

    /// Creates a new Dynarmic instance for ARM32 (A32) emulation.
    ///
    /// The JIT cache size can be configured via the `DYNARMIC_JIT_SIZE` environment variable (in MB).
    /// Defaults to 64MB. Valid range: 8MB to 512MB.
    pub fn new_a32() -> Dynarmic<'static, T> {
        let memory = unsafe { ffi::dynarmic_init_memory() };
        if memory == null_mut() {
            error!("Failed to initialize memory");
            exit(0)
        }

        let mut jit_size = std::env::var("DYNARMIC_JIT_SIZE")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(64);

        if jit_size < 8 {
            warn!("JIT size {}MB is too small, setting to 8MB", jit_size);
            jit_size = 8;
        } else if jit_size > 512 {
            warn!("JIT size {}MB is too large, setting to 512MB", jit_size);
            jit_size = 512;
        }

        let monitor = unsafe { ffi::dynarmic_init_monitor(1) };
        let page_table = unsafe { ffi::dynarmic_init_page_table() };
        let handle = unsafe {
            ffi::dynarmic_new_a32(
                0,
                memory,
                monitor,
                page_table,
                jit_size * 1024 * 1024,
                true,
                null_mut(),
            )
        };

        debug!(
            "[Dynarmic] Created new Dynarmic A32 instance: {:X} with {}MB JIT",
            handle as usize, jit_size
        );

        Dynarmic {
            cur_handle: handle,
            metadata: Arc::new(UnsafeCell::new(Metadata {
                svc_callback: None,
                unmapped_mem_callback: None,
                until: 0,
                _memory: memory,
                _monitor: monitor,
                _page_table: page_table,
                handle,
            })),
            pd: PhantomData,
        }
    }

    /// Starts emulation from the given PC until the `until` address is reached.
    ///
    /// * `pc`: The starting Program Counter address.
    /// * `until`: The address at which to stop execution.
    pub fn emu_start(&self, pc: u64, until: u64) -> anyhow::Result<()> {
        self.emu_start_bounded(pc, until, 0x10000000000)
    }

    pub fn emu_start_bounded(&self, pc: u64, until: u64, ticks: u64) -> anyhow::Result<()> {
        unsafe {
            debug!("[Dynarmic] Starting emulator: pc=0x{:x}", pc);

            // saturating_add so callers can pass u64::MAX as a sentinel
            // "no synthetic stop" without overflowing.
            (*self.metadata.get()).until = until.saturating_add(4);

            let ret = ffi::dynarmic_emu_start_bounded(self.cur_handle, pc, ticks.max(1));
            if ret != 0 {
                return Err(anyhow!("Failed to start emulator: code={}", ret));
            }
            Ok(())
        }
    }

    pub fn emu_ticks_remaining(&self) -> u64 {
        unsafe { ffi::dynarmic_emu_ticks_remaining(self.cur_handle) }
    }

    /// Stops the emulation.
    pub fn emu_stop(&self) -> anyhow::Result<()> {
        unsafe {
            debug!("[Dynarmic] Stopping emulator");
            let ret = ffi::dynarmic_emu_stop(self.cur_handle);
            if ret != 0 {
                return Err(anyhow!("Failed to stop emulator: code={}", ret));
            }
            Ok(())
        }
    }

    /// Returns the current size of the JIT code cache.
    pub fn get_cache_size(&self) -> u64 {
        unsafe { ffi::dynarmic_get_cache_size(self.cur_handle) }
    }

    pub fn get_cache_capacity(&self) -> u64 {
        unsafe { ffi::dynarmic_get_cache_capacity(self.cur_handle) }
    }

    pub fn get_cache_evacuation_count(&self) -> u64 {
        unsafe { ffi::dynarmic_get_cache_evacuation_count(self.cur_handle) }
    }

    pub fn invalidate_cache_range(&self, address: u64, size: u64) {
        unsafe { ffi::dynarmic_invalidate_cache_range(self.cur_handle, address, size) }
    }

    pub fn clear_cache(&self) {
        unsafe { ffi::dynarmic_clear_cache(self.cur_handle) }
    }

    /// Allocates a new ARM64 context.
    pub fn context_alloc(&self) -> DynarmicContext {
        unsafe {
            let inner_context = ffi::dynarmic_context_alloc();
            Arc::new(DynarmicContextInner { inner_context })
        }
    }

    /// Saves the current ARM64 CPU state into the provided context.
    pub fn context_save(&self, context: &mut DynarmicContext) -> anyhow::Result<()> {
        unsafe {
            let ret = ffi::dynarmic_context_save(self.cur_handle, context.inner_context);
            if ret != 0 {
                return Err(anyhow!("Failed to save context: code={}", ret));
            }
            Ok(())
        }
    }

    /// Restores the ARM64 CPU state from the provided context.
    pub fn context_restore(&self, context: &DynarmicContext) -> anyhow::Result<()> {
        unsafe {
            let ret = ffi::dynarmic_context_restore(self.cur_handle, context.inner_context);
            if ret != 0 {
                return Err(anyhow!("Failed to restore context: code={}", ret));
            }
            Ok(())
        }
    }

    /// Allocates a new ARM32 context.
    pub fn context32_alloc(&self) -> DynarmicContext32 {
        unsafe {
            let inner_context = ffi::dynarmic_context32_alloc();
            Arc::new(DynarmicContext32Inner { inner_context })
        }
    }

    /// Saves the current ARM32 CPU state into the provided context.
    pub fn context32_save(&self, context: &mut DynarmicContext32) -> anyhow::Result<()> {
        unsafe {
            let ret = ffi::dynarmic_context32_save(self.cur_handle, context.inner_context);
            if ret != 0 {
                return Err(anyhow!("Failed to save A32 context: code={}", ret));
            }
            Ok(())
        }
    }

    /// Restores the ARM32 CPU state from the provided context.
    pub fn context32_restore(&self, context: &DynarmicContext32) -> anyhow::Result<()> {
        unsafe {
            let ret = ffi::dynarmic_context32_restore(self.cur_handle, context.inner_context);
            if ret != 0 {
                return Err(anyhow!("Failed to restore A32 context: code={}", ret));
            }
            Ok(())
        }
    }

    /// Maps a region of memory for the emulator.
    ///
    /// * `addr`: The guest virtual address.
    /// * `size`: The size of the region in bytes (must be page-aligned).
    /// * `prot`: Protection flags (1: Read, 2: Write, 4: Execute).
    pub fn mem_map(&self, addr: u64, size: usize, prot: u32) -> anyhow::Result<()> {
        unsafe {
            debug!(
                "[Dynarmic] Mapping memory: addr=0x{:x}, size=0x{:x}, prot={}",
                addr, size, prot
            );
            let ret =
                ffi::dynarmic_mmap(self.cur_handle, addr, size as u64, u32::cast_signed(prot));
            if ret == 4 {
                warn!("Replace mmap?");
            }
            if ret != 0 {
                return Err(anyhow!("Failed to map memory: code={}", ret));
            }
            debug!(
                "[Dynarmic] Mapped memory: addr=0x{:x}, size=0x{:x}, prot={}",
                addr, size, prot
            );
            Ok(())
        }
    }

    /// Maps a region of memory using a host pointer.
    pub fn mem_map_ptr(
        &self,
        addr: u64,
        size: usize,
        prot: u32,
        ptr: *mut c_void,
    ) -> anyhow::Result<()> {
        unsafe {
            debug!(
                "[Dynarmic] Mapping memory ptr: addr=0x{:x}, size=0x{:x}, prot={}, ptr={:?}",
                addr, size, prot, ptr
            );
            let ret = ffi::dynarmic_mem_map_ptr(
                self.cur_handle,
                addr,
                size as u64,
                u32::cast_signed(prot),
                ptr,
            );
            if ret == 4 {
                warn!("Replace mmap?");
            }
            if ret != 0 {
                return Err(anyhow!("Failed to map memory ptr: code={}", ret));
            }
            debug!(
                "[Dynarmic] Mapped memory ptr: addr=0x{:x}, size=0x{:x}, prot={}",
                addr, size, prot
            );
            Ok(())
        }
    }

    /// Unmaps a region of memory.
    pub fn mem_unmap(&self, addr: u64, size: usize) -> anyhow::Result<()> {
        unsafe {
            debug!(
                "[Dynarmic] Unmapping memory: addr=0x{:x}, size=0x{:x}",
                addr, size
            );
            let ret = ffi::dynarmic_munmap(self.cur_handle, addr, size as u64);
            if ret != 0 {
                return Err(anyhow!("Failed to unmap memory: code={}", ret));
            }
            Ok(())
        }
    }

    /// Updates protection flags for a memory region.
    pub fn mem_protect(&self, addr: u64, size: usize, prot: u32) -> anyhow::Result<()> {
        unsafe {
            debug!(
                "[Dynarmic] Protecting memory: addr=0x{:x}, size=0x{:x}, prot={}",
                addr, size, prot
            );
            let ret = ffi::dynarmic_mem_protect(
                self.cur_handle,
                addr,
                size as u64,
                u32::cast_signed(prot),
            );
            if ret != 0 {
                return Err(anyhow!("Failed to protect memory: code={}", ret));
            }
            Ok(())
        }
    }

    /// Reads a 64-bit register value (ARM64).
    pub fn reg_read(&self, index: usize) -> anyhow::Result<u64> {
        unsafe { Ok(ffi::reg_read(self.cur_handle, index as u64)) }
    }

    /// Reads the Link Register (X30 in ARM64).
    pub fn reg_read_lr(&self) -> anyhow::Result<u64> {
        unsafe { Ok(ffi::reg_read(self.cur_handle, 30)) }
    }

    /// Reads the NZCV (flags) register.
    pub fn reg_read_nzcv(&self) -> anyhow::Result<u64> {
        unsafe { Ok(ffi::reg_read_nzcv(self.cur_handle)) }
    }

    /// Reads the Stack Pointer.
    pub fn reg_read_sp(&self) -> anyhow::Result<u64> {
        unsafe { Ok(ffi::reg_read_sp(self.cur_handle)) }
    }

    /// Reads the Thread ID Register (EL0).
    pub fn reg_read_tpidr_el0(&self) -> anyhow::Result<u64> {
        unsafe { Ok(ffi::reg_read_tpidr_el0(self.cur_handle)) }
    }

    /// Reads the Program Counter.
    pub fn reg_read_pc(&self) -> anyhow::Result<u64> {
        unsafe { Ok(ffi::reg_read_pc(self.cur_handle)) }
    }

    /// Writes the Program Counter.
    pub fn reg_write_pc(&self, value: u64) -> anyhow::Result<()> {
        unsafe {
            debug!("[Dynarmic] Writing PC: value=0x{:x}", value);
            let ret = ffi::reg_write_pc(self.cur_handle, value);
            if ret != 0 {
                return Err(anyhow!("Failed to write PC: code={}", ret));
            }
            Ok(())
        }
    }

    /// Writes the Stack Pointer.
    pub fn reg_write_sp(&self, value: u64) -> anyhow::Result<()> {
        unsafe {
            debug!("[Dynarmic] Writing SP: value=0x{:x}", value);
            let ret = ffi::reg_write_sp(self.cur_handle, value);
            if ret != 0 {
                return Err(anyhow!("Failed to write SP: code={}", ret));
            }
            Ok(())
        }
    }

    /// Writes the Link Register.
    pub fn reg_write_lr(&self, value: u64) -> anyhow::Result<()> {
        unsafe {
            debug!("[Dynarmic] Writing LR: value=0x{:x}", value);
            let ret = ffi::reg_write(self.cur_handle, 30, value);
            if ret != 0 {
                return Err(anyhow!("Failed to write LR: code={}", ret));
            }
            Ok(())
        }
    }

    /// Writes the Thread ID Register (EL0).
    pub fn reg_write_tpidr_el0(&self, value: u64) -> anyhow::Result<()> {
        unsafe {
            debug!("[Dynarmic] Writing TPIDR_EL0: value=0x{:x}", value);
            let ret = ffi::reg_write_tpidr_el0(self.cur_handle, value);
            if ret != 0 {
                return Err(anyhow!("Failed to write TPIDR_EL0: code={}", ret));
            }
            Ok(())
        }
    }

    /// Writes the Thread ID Read-Only Register (EL0). On real ARM64 this is
    /// a separate system register from `TPIDR_EL0` (R/W) and is read by
    /// `MRS Xn, TPIDRRO_EL0`. Prior to 0.2.1 this method incorrectly aliased
    /// to `reg_write_tpidr_el0`, so guests reading `TPIDRRO_EL0` always saw
    /// 0 — breaking any TLS scheme (libnx) that uses that register.
    pub fn reg_write_tpidrr0_el0(&self, value: u64) -> anyhow::Result<()> {
        unsafe {
            ffi::reg_write_tpidrr0_el0(self.cur_handle, value);
        }
        Ok(())
    }

    /// Reads the Thread ID Read-Only Register (EL0). Returns whatever was
    /// last written via `reg_write_tpidrr0_el0`; distinct from `TPIDR_EL0`.
    pub fn reg_read_tpidrr0_el0(&self) -> anyhow::Result<u64> {
        unsafe { Ok(ffi::reg_read_tpidrr0_el0(self.cur_handle)) }
    }

    /// Writes the NZCV (flags) register.
    pub fn reg_write_nzcv(&self, value: u64) -> anyhow::Result<()> {
        unsafe {
            debug!("[Dynarmic] Writing NZCV: value=0x{:x}", value);
            let ret = ffi::reg_write_nzcv(self.cur_handle, value);
            if ret != 0 {
                return Err(anyhow!("Failed to write NZCV: code={}", ret));
            }
            Ok(())
        }
    }

    /// Writes a generic register value by index.
    pub fn reg_write_raw(&self, index: usize, value: u64) -> anyhow::Result<()> {
        unsafe {
            debug!(
                "[Dynarmic] Writing register: index={}, value=0x{:x}",
                index, value
            );
            let ret = ffi::reg_write(self.cur_handle, index as u64, value);
            if ret != 0 {
                return Err(anyhow!("Failed to write register: code={}", ret));
            }
            Ok(())
        }
    }

    /// Writes to CP15 c13 0 3 (ARM32 Thread ID).
    pub fn reg_write_c13_c0_3(&self, value: u32) -> anyhow::Result<()> {
        unsafe {
            debug!("[Dynarmic] Writing CP15 c13 0 3: value=0x{:x}", value);
            let ret = ffi::reg_write_c13_c0_3(self.cur_handle, value);
            if ret != 0 {
                return Err(anyhow!("Failed to write CP15 c13 0 3: code={}", ret));
            }
            Ok(())
        }
    }

    /// Reads from CP15 c13 0 3 (ARM32 Thread ID).
    pub fn reg_read_c13_c0_3(&self) -> anyhow::Result<u32> {
        unsafe { Ok(ffi::reg_read_c13_c0_3(self.cur_handle)) }
    }

    /// Writes an ARM32 general-purpose register.
    pub fn reg_write_r(&self, index: u32, value: u32) -> anyhow::Result<()> {
        unsafe {
            let ret = ffi::reg_write_r(self.cur_handle, index, value);
            if ret != 0 {
                return Err(anyhow!("Failed to write R register: code={}", ret));
            }
            Ok(())
        }
    }

    /// Reads an ARM32 general-purpose register.
    pub fn reg_read_r(&self, index: u32) -> anyhow::Result<u32> {
        unsafe { Ok(ffi::reg_read_r(self.cur_handle, index)) }
    }

    /// Writes the CPSR register (ARM32).
    pub fn reg_write_cpsr(&self, value: u32) -> anyhow::Result<()> {
        unsafe {
            let ret = ffi::reg_write_cpsr(self.cur_handle, value);
            if ret != 0 {
                return Err(anyhow!("Failed to write CPSR: code={}", ret));
            }
            Ok(())
        }
    }

    /// Reads the CPSR register (ARM32).
    pub fn reg_read_cpsr(&self) -> anyhow::Result<u32> {
        unsafe { Ok(ffi::reg_read_cpsr(self.cur_handle)) }
    }

    /// Reads a null-terminated C string from guest memory.
    pub fn mem_read_c_string(&self, mut addr: u64) -> anyhow::Result<String> {
        let mut buf = Vec::new();
        let mut byte = [0u8];
        loop {
            self.mem_read(addr, &mut byte)?;
            if byte[0] == 0 {
                break;
            }
            buf.push(byte[0]);
            addr += 1;
        }
        unsafe { Ok(String::from_utf8_unchecked(buf)) }
    }

    /// Reads guest memory into a mutable buffer.
    pub fn mem_read(&self, addr: u64, dest: &mut [u8]) -> anyhow::Result<()> {
        unsafe {
            debug!(
                "[Dynarmic] Reading memory: addr=0x{:x}, size=0x{:x}",
                addr,
                dest.len()
            );
            let ret = ffi::dynarmic_mem_read(
                self.cur_handle,
                addr,
                dest.as_mut_ptr() as *mut _,
                dest.len(),
            );
            if ret != 0 {
                return Err(anyhow!("Failed to read memory: code={}", ret));
            }
            Ok(())
        }
    }

    /// Reads guest memory and returns it as a Vec<u8>.
    pub fn mem_read_as_vec(&self, addr: u64, size: usize) -> anyhow::Result<Vec<u8>> {
        let mut buf = vec![0; size];
        self.mem_read(addr, &mut buf)?;
        Ok(buf)
    }

    /// Writes a buffer into guest memory.
    pub fn mem_write(&self, addr: u64, value: &[u8]) -> anyhow::Result<()> {
        unsafe {
            debug!(
                "[Dynarmic] Writing memory: addr=0x{:x}, size=0x{:x}",
                addr,
                value.len()
            );
            let ret = ffi::dynarmic_mem_write(
                self.cur_handle,
                addr,
                value.as_ptr() as *const _,
                value.len(),
            );
            if ret != 0 {
                return Err(anyhow!("Failed to write memory: code={}", ret));
            }
            Ok(())
        }
    }

    /// Sets a callback for SVC (Supervisor Call) instructions.
    ///
    /// The callback receives the emulator instance, the SWI number, the end address, and the current PC.
    pub fn set_svc_callback<F: 'a>(&self, callback: F)
    where
        F: FnMut(&Dynarmic<T>, u32, u64, u64),
    {
        debug!("[Dynarmic] Setting SVC callback");
        unsafe {
            let mut cb = Box::new(DyHook {
                callback,
                dy: self.clone(),
            });
            let user_data = cb.as_mut() as *mut _ as *const c_void;

            extern "C" fn svc_callback_wrapper<T: Clone + Send + Sync, F>(
                swi: u32,
                user_data: *const c_void,
            ) where
                F: FnMut(&Dynarmic<T>, u32, u64, u64),
            {
                if swi == 114514 {
                    return;
                }
                unsafe {
                    let cb = &mut *(user_data as *mut DyHook<T, F>);
                    let dynarmic = &cb.dy;
                    let pc = ffi::reg_read_pc(dynarmic.cur_handle);
                    let until = (*dynarmic.metadata.get()).until;
                    (cb.callback)(dynarmic, swi, until, pc);
                }
            }

            ffi::dynarmic_set_svc_callback(
                self.cur_handle,
                svc_callback_wrapper::<T, F>,
                user_data,
            );
            (*self.metadata.get()).svc_callback = Some(cb);
        }
    }

    /// Sets a callback for unmapped memory accesses.
    ///
    /// The callback should return `true` if the access was handled, `false` otherwise.
    pub fn set_unmapped_mem_callback<F: 'a>(&self, callback: F)
    where
        F: FnMut(&Dynarmic<T>, u64, usize, u64) -> bool,
    {
        debug!("[Dynarmic] Setting unmapped memory callback");
        unsafe {
            let mut cb = Box::new(DyHook {
                callback,
                dy: self.clone(),
            });
            let user_data = cb.as_mut() as *mut _ as *const c_void;

            extern "C" fn unmapped_mem_callback_wrapper<T: Clone + Send + Sync, F>(
                addr: u64,
                size: usize,
                value: u64,
                user_data: *const c_void,
            ) -> bool
            where
                F: FnMut(&Dynarmic<T>, u64, usize, u64) -> bool,
            {
                unsafe {
                    let cb = &mut *(user_data as *mut DyHook<T, F>);
                    let dynarmic = &cb.dy;
                    (cb.callback)(dynarmic, addr, size, value)
                }
            }

            ffi::dynarmic_set_unmapped_mem_callback(
                self.cur_handle,
                unmapped_mem_callback_wrapper::<T, F>,
                user_data,
            );
            (*self.metadata.get()).unmapped_mem_callback = Some(cb);
        }
    }

    /// Clears and removes all active callbacks.
    pub fn destroy_callback(&self) {
        unsafe {
            extern "C" fn empty_svc_callback(_: u32, _: *const c_void) {}
            extern "C" fn empty_unmapped_callback(
                _: u64,
                _: usize,
                _: u64,
                _: *const c_void,
            ) -> bool {
                false
            }

            ffi::dynarmic_set_svc_callback(self.cur_handle, empty_svc_callback, null_mut());
            let callback = (*self.metadata.get()).svc_callback.take();
            drop(callback);

            ffi::dynarmic_set_unmapped_mem_callback(
                self.cur_handle,
                empty_unmapped_callback,
                null_mut(),
            );
            let callback = (*self.metadata.get()).unmapped_mem_callback.take();
            drop(callback);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        code_page_cache_enabled, guarded_fast_paths_enabled, parse_default_on_flag,
        shared_fastmem_page_table_enabled, unsafe_fastmem_enabled, Dynarmic, FastmemPageTableRoute,
        FastmemPageTableSlot,
    };
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    const CODE_ADDR: u64 = 0x0100_0000;
    const MAP_SIZE: usize = 0x1000;
    const LONG_BUDGET: u64 = 1_000_000_000_000;

    fn emulator_with_code(code: &[u8]) -> Dynarmic<'static, ()> {
        let emu: Dynarmic<'static, ()> = Dynarmic::new();
        emu.mem_map(CODE_ADDR, MAP_SIZE, 7).expect("map test code");
        emu.mem_write(CODE_ADDR, code).expect("write test code");
        emu
    }

    #[test]
    fn fast_path_switch_matches_environment() {
        let expected = std::env::var("DYNARMIC_FAST_PATHS")
            .ok()
            .is_some_and(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "on" | "yes"
                )
            });
        assert_eq!(guarded_fast_paths_enabled(), expected);
    }

    #[test]
    fn parity_switches_match_environment() {
        let code_cache_expected =
            parse_default_on_flag(std::env::var("DYNARMIC_CODE_PAGE_CACHE").ok().as_deref());
        let unsafe_fastmem_expected = std::env::var("NEXIUM_DYNARMIC_UNSAFE_FASTMEM")
            .ok()
            .is_some_and(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "on" | "yes"
                )
            });
        let shared_page_table_expected = std::env::var("NEXIUM_DYNARMIC_SHARED_PAGE_TABLE")
            .ok()
            .is_some_and(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "on" | "yes"
                )
            });
        assert_eq!(code_page_cache_enabled(), code_cache_expected);
        assert_eq!(unsafe_fastmem_enabled(), unsafe_fastmem_expected);
        assert_eq!(
            shared_fastmem_page_table_enabled(),
            shared_page_table_expected
        );
    }

    #[test]
    fn shared_page_table_slot_reuses_only_the_same_base() {
        let mut slot = FastmemPageTableSlot::default();
        let (first, first_route) = slot.get_or_allocate(0x1000, || 0xaaaa);
        let (same, same_route) = slot.get_or_allocate(0x1000, || panic!("must reuse"));
        let (different, different_route) = slot.get_or_allocate(0x2000, || 0xbbbb);

        assert_eq!(first, 0xaaaa);
        assert_eq!(first_route, FastmemPageTableRoute::InitializedShared);
        assert_eq!(same, first);
        assert_eq!(same_route, FastmemPageTableRoute::ReusedShared);
        assert_eq!(different, 0xbbbb);
        assert_eq!(
            different_route,
            FastmemPageTableRoute::PrivateForDifferentBase
        );
    }

    #[test]
    fn code_fetch_tracks_remap_and_explicit_invalidation() {
        fn svc_instruction(immediate: u16) -> [u8; 4] {
            (0xd400_0001u32 | (u32::from(immediate) << 5)).to_le_bytes()
        }

        let emu: Dynarmic<'static, ()> = Dynarmic::new();
        let mut first_page = vec![0u8; MAP_SIZE];
        first_page[..4].copy_from_slice(&svc_instruction(1));
        emu.mem_map_ptr(CODE_ADDR, MAP_SIZE, 7, first_page.as_mut_ptr().cast())
            .expect("map first host code page");

        let (svc_tx, svc_rx) = mpsc::channel();
        emu.set_svc_callback(move |dynarmic, immediate, _, _| {
            svc_tx.send(immediate).expect("report SVC");
            dynarmic.emu_stop().expect("stop at SVC");
        });

        emu.emu_start_bounded(CODE_ADDR, u64::MAX - 16, 64)
            .expect("execute first host page");
        assert_eq!(svc_rx.recv().expect("first SVC"), 1);

        emu.mem_unmap(CODE_ADDR, MAP_SIZE)
            .expect("unmap first host page");
        let mut second_page = vec![0u8; MAP_SIZE];
        second_page[4..8].copy_from_slice(&svc_instruction(2));
        emu.mem_map_ptr(CODE_ADDR, MAP_SIZE, 7, second_page.as_mut_ptr().cast())
            .expect("map replacement host code page");

        emu.emu_start_bounded(CODE_ADDR + 4, u64::MAX - 16, 64)
            .expect("execute replacement host page");
        assert_eq!(svc_rx.recv().expect("replacement SVC"), 2);

        second_page[4..8].copy_from_slice(&svc_instruction(3));
        emu.invalidate_cache_range(CODE_ADDR + 4, 4);
        emu.emu_start_bounded(CODE_ADDR + 4, u64::MAX - 16, 64)
            .expect("execute range-invalidated code");
        assert_eq!(svc_rx.recv().expect("range-invalidated SVC"), 3);

        second_page[4..8].copy_from_slice(&svc_instruction(4));
        emu.clear_cache();
        emu.emu_start_bounded(CODE_ADDR + 4, u64::MAX - 16, 64)
            .expect("execute full-cache-invalidated code");
        assert_eq!(svc_rx.recv().expect("full-cache-invalidated SVC"), 4);

        assert!(emu.get_cache_capacity() >= 8 * 1024 * 1024);
        assert_eq!(emu.get_cache_evacuation_count(), 0);
    }

    fn install_run_marker(emu: &Dynarmic<'static, ()>) -> mpsc::Receiver<()> {
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        emu.set_svc_callback(move |_, _, _, _| {
            entered_tx.send(()).expect("announce guest entry");
        });
        entered_rx
    }

    fn assert_external_stop_returns(
        emu: &Dynarmic<'static, ()>,
        run_pc: u64,
        entered_rx: mpsc::Receiver<()>,
    ) {
        let runner = emu.clone();
        let (done_tx, done_rx) = mpsc::sync_channel(0);
        let handle = std::thread::spawn(move || {
            let result = runner.emu_start_bounded(run_pc, u64::MAX - 16, LONG_BUDGET);
            let remaining = runner.emu_ticks_remaining();
            done_tx
                .send((result, remaining))
                .expect("report runner completion");
        });

        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("guest did not reach the run marker");
        std::thread::sleep(Duration::from_millis(25));
        let stop_at = Instant::now();
        emu.emu_stop().expect("request external stop");
        let (result, remaining) = done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("fast-path execution ignored HaltExecution");
        result.expect("bounded execution failed");
        assert!(
            stop_at.elapsed() < Duration::from_secs(2),
            "external halt exceeded its bounded latency"
        );
        assert!(
            remaining > 0,
            "external halt consumed the entire long budget"
        );
        handle.join().expect("runner thread panicked");
    }

    #[test]
    fn fast_dispatch_honors_budget_and_external_halt() {
        let code = [
            0x01, 0x00, 0x00, 0xd4, 0x21, 0x04, 0x00, 0x91, 0x00, 0x00, 0x1f, 0xd6,
        ];
        let emu = emulator_with_code(&code);
        let loop_pc = CODE_ADDR + 4;
        emu.reg_write_raw(0, loop_pc).expect("set indirect target");

        const BUDGET: u64 = 64;
        emu.emu_start_bounded(loop_pc, u64::MAX - 16, BUDGET)
            .expect("run bounded fast-dispatch loop");
        assert_eq!(
            emu.emu_ticks_remaining(),
            0,
            "fast dispatch returned before exhausting the small budget"
        );
        assert!(
            (1..=BUDGET).contains(&emu.reg_read(1).expect("read loop counter")),
            "fast dispatch grossly exceeded the bounded budget"
        );

        emu.reg_write_raw(1, 0).expect("reset loop counter");
        let entered_rx = install_run_marker(&emu);
        assert_external_stop_returns(&emu, CODE_ADDR, entered_rx);
    }

    #[test]
    fn return_stack_buffer_honors_external_halt() {
        let code = [
            0x01, 0x00, 0x00, 0xd4, 0x02, 0x00, 0x00, 0x94, 0xff, 0xff, 0xff, 0x17, 0x21, 0x04,
            0x00, 0x91, 0xc0, 0x03, 0x5f, 0xd6,
        ];
        let emu = emulator_with_code(&code);
        let entered_rx = install_run_marker(&emu);
        assert_external_stop_returns(&emu, CODE_ADDR, entered_rx);
    }

    #[test]
    fn ticks_remaining_reports_early_svc_exit() {
        let emu = emulator_with_code(&[0x01, 0x00, 0x00, 0xd4]);
        emu.set_svc_callback(|dynarmic, _, _, _| {
            dynarmic.emu_stop().expect("stop from SVC callback");
        });

        const BUDGET: u64 = 10_000;
        emu.emu_start_bounded(CODE_ADDR, u64::MAX - 16, BUDGET)
            .expect("run SVC");
        let remaining = emu.emu_ticks_remaining();
        assert!(
            remaining > 0 && remaining < BUDGET,
            "early SVC exit did not preserve an exact remaining budget: {remaining}"
        );
    }
}
