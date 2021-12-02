#![feature(asm)]

use goblin::elf::program_header::program_header64::ProgramHeader;
use goblin::{elf::Elf, error, Object};
use libc::{c_void, mmap, MAP_ANONYMOUS, MAP_PRIVATE};
use std::os::raw::c_char;
use std::path::Path;

const AT_NULL: usize = 0;
const AT_PHDR: usize = 3;
const AT_PHENT: usize = 4;
const AT_PHNUM: usize = 5;
const AT_BASE: usize = 7;
const AT_ENTRY: usize = 9;

#[repr(C)]
#[derive(Debug)]
struct ElfAuxv {
    key: usize,
    value: usize,
}

#[derive(Debug)]
struct Stack {
    argc: *mut usize,
    argv: &'static mut [*const c_char],
    _envp: &'static mut [*const c_char],
    auxv: &'static mut [ElfAuxv],
}

impl Stack {
    fn as_ptr(&self) -> *const c_void {
        self.argc as *const c_void
    }
}

// Get pointers to the initial stack frame for manipulation and reuse in the SHELF
fn get_initial_stack() -> Stack {
    // Use environ from libc to find envp.
    // Technique taken from https://docs.rs/auxv/latest/src/auxv/stack.rs.html#69
    extern "C" {
        static environ: *const *const c_char;
    }
    let envp = unsafe { environ } as *mut *const c_char;

    // As detailed @ https://articles.manugarg.com/aboutelfauxiliaryvectors.html.
    // The initial stack looks like:
    // position            content                     size (bytes) + comment
    // ------------------------------------------------------------------------
    // stack pointer ->  [ argc = number of args ]     4
    //                   [ argv[0]  (pointer) ]        4   (program name)
    //                   [ argv[..] (pointer) ]        4
    //                   [ argv[n]  (pointer) ]        4   (= NULL)
    //                   [ envp[0]  (pointer) ]        4
    //                   [ envp[..] (pointer) ]        4
    //                   [ envp[m]  (pointer) ]        4   (= NULL)
    //                   [ auxv[0]  (Elf32_auxv_t) ]   8
    //                   [ auxv[1]  (Elf32_auxv_t) ]   8
    //                   [ auxv[..] (Elf32_auxv_t) ]   8
    //                   [ auxv[l]  (Elf32_auxv_t) ]   8   (= AT_NULL vector)
    //                   [ padding ]                   0 - 16
    //                   [ argument ASCIIZ strings ]   >= 0
    //                   [ environment ASCIIZ str. ]   >= 0
    //   (0xbffffffc)    [ end marker ]                4   (= NULL)
    //   (0xc0000000)    < bottom of stack >           0   (virtual)
    //   ------------------------------------------------------------------------
    //
    // As we already know envp, we can search for the null terminator to find auxv.
    // This technique was also learned from the auxv crate.
    // To find argv and argc we can traverse down the stack (up the graphic) until the value at our
    // pointer equals the number of args we have iterated over.

    // Find auxv and get the size of envp
    let mut auxv = envp;
    let mut envp_len = 0;
    unsafe {
        // Increment until we find the end of envp
        while !(*auxv).is_null() {
            auxv = auxv.add(1);
            envp_len += 1;
        }
        // Add one for the NULL
        auxv = auxv.add(1);
        envp_len += 1;
    }
    let auxv = auxv as *mut ElfAuxv;

    // Find the auxv length
    let auxv_len = unsafe {
        let mut auxv_iter = auxv;
        let mut len = 0;
        while (*auxv_iter).key != AT_NULL {
            auxv_iter = auxv_iter.add(1);
            len += 1;
        }
        // Add one for the AT_NULL
        len + 1
    };

    // Find argv and argc
    let mut argc: *mut usize = unsafe { envp.sub(2) as *mut usize };
    let mut argv_len = 0;
    let argv = unsafe {
        // This may fail if strings in argv are placed in a very low page and argv is huge
        // ... but that sounds unlikely and like someone else's problem
        while *argc != argv_len {
            argc = argc.sub(1);
            argv_len += 1;
        }
        argv_len += 1;
        argc.add(1) as *mut *const c_char
    };
    unsafe {
        Stack {
            argc,
            argv: std::slice::from_raw_parts_mut(argv, argv_len),
            _envp: std::slice::from_raw_parts_mut(envp, envp_len),
            auxv: std::slice::from_raw_parts_mut(auxv, auxv_len),
        }
    }
}

fn setup_auxv(auxv: &mut [ElfAuxv], phdr: *const c_void, phnum: usize, entry: *const c_void) {
    // Update auxv values with our SHELF's new values
    let phentsize = goblin::elf::program_header::program_header64::SIZEOF_PHDR;
    for aux in auxv {
        match aux.key {
            AT_PHDR => aux.value = phdr as usize,
            AT_PHNUM => aux.value = phnum,
            AT_PHENT => aux.value = phentsize,
            AT_BASE => aux.value = 0,
            AT_ENTRY => aux.value = entry as usize,
            _ => (),
        }
    }
}

fn exec_shelf(entry: *mut c_void, stack: &Stack) -> ! {
    unsafe {
        asm!(
            "mov rsp, {0}",
            "jmp {1}",
            in(reg) stack.as_ptr(),
            in(reg) entry,
            options(noreturn),
        );
    }
}

fn process_elf(elf: &Elf, raw_file: &[u8]) {
    let mut load_phdr: Option<&goblin::elf::ProgramHeader> = None;

    // Get relevant headers. We only load TLS and DYNAMIC segment headers into SHELF memory
    let mut phdrs: Vec<ProgramHeader> = vec![];
    for h in elf.program_headers.iter() {
        match h.p_type {
            goblin::elf::program_header::PT_LOAD => load_phdr = Some(h),
            goblin::elf::program_header::PT_TLS | goblin::elf::program_header::PT_DYNAMIC => {
                phdrs.push(ProgramHeader::from(h.clone()))
            }
            _ => (),
        }
    }
    let load_phdr = load_phdr.expect("No loadable segments");

    // Load the loadable segment
    // Should probably verify segment contents
    let load_vaddr: usize = load_phdr.p_vaddr.try_into().unwrap();
    let load_offset: usize = load_phdr.p_offset.try_into().unwrap();
    let load_filesz: usize = load_phdr.p_filesz.try_into().unwrap();
    let load_perms: i32 = load_phdr.p_flags.try_into().unwrap();
    let mem_size: usize = load_phdr.p_memsz.try_into().unwrap();
    let (mapping, entry) = unsafe {
        let mapping = mmap(
            std::ptr::null_mut(),
            mem_size + load_vaddr,
            load_perms,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        );
        println!("mapping: {:?}", mapping);
        // Copy the loadable segment
        let src: &[u8] = &raw_file[load_offset..load_offset + load_filesz];
        std::ptr::copy_nonoverlapping(src.as_ptr(), mapping.add(load_vaddr) as *mut u8, src.len());

        (mapping, mapping.add(elf.entry.try_into().unwrap()))
    };
    println!("entry: {:?}", entry);

    // Copy in the phdrs at the start of the LOAD segment
    let phnum = phdrs.len();
    let phdrs = unsafe {
        let dst_phdrs_ptr = mapping.add(elf.header.e_phoff.try_into().unwrap());
        let dst_phdrs = std::slice::from_raw_parts_mut(dst_phdrs_ptr as *mut ProgramHeader, phnum);
        dst_phdrs.copy_from_slice(&phdrs);
        dst_phdrs_ptr
    };

    // Get the stack so that we can edit it and pass to the SHELF
    let mut stack = get_initial_stack();
    setup_auxv(stack.auxv, phdrs, phnum, entry);

    // Edit argv by passing argv[1..] to the SHELF
    unsafe {
        stack.argv =
            std::slice::from_raw_parts_mut(stack.argv.as_mut_ptr().add(1), stack.argv.len() - 1);
        stack.argc = stack.argc.add(1);
        // Subtract 1 as argc doesn't include the NULL
        *stack.argc = stack.argv.len() - 1;
    }
    println!("Starting SHELF...");
    exec_shelf(entry, &stack);
}

fn main() -> error::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        println!("Usage: shelf-loader-poc <SHELF> <ARGS>");
        return Ok(());
    }
    let arg = args.get(1).expect("no SHELF given");
    let path = Path::new(&arg);
    let buffer = std::fs::read(path)?;
    match Object::parse(&buffer)? {
        Object::Elf(elf) => {
            process_elf(&elf, &buffer);
        }
        _ => {
            println!("filetype not supported");
        }
    }
    Ok(())
}
