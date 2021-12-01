#![feature(asm)]

use auxv::AuxvPair;
use libc::{
    c_void,
    mmap,
    MAP_ANONYMOUS, MAP_PRIVATE, MAP_STACK,
};
use goblin::{error,Object,elf::Elf};
use goblin::elf::program_header::program_header64::ProgramHeader;
use std::path::Path;
use std::ffi::CString;

const AT_NULL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_BASE: u64 = 7;
const AT_ENTRY: u64 = 9;

fn setup_auxv(auxv: &mut[AuxvPair], phdr: *const c_void, phnum: u64, entry: *const c_void) {
    let phentsize =  goblin::elf::program_header::program_header64::SIZEOF_PHDR;
    for i in 0..auxv.len() {
        match auxv[i].key {
            AT_PHDR => auxv[i].value = phdr as u64,
            AT_PHNUM => auxv[i].value = phnum,
            AT_PHENT => auxv[i].value = phentsize as u64,
            AT_BASE => auxv[i].value = 0,
            AT_ENTRY => auxv[i].value = entry as u64,
            _ => (),
        }
    }
}

fn exec_shelf(entry: *mut c_void, stack_top: *mut c_void, argv:&[CString], auxv: &[AuxvPair]) -> ! {
    // My OS sets up the stack to look like
    // RSP     -> argc
    // RSP+8   -> argv
    // RSP+N   -> envp
    // RSP+N+X -> auxv
    
    // Plus one for the AT_NULL entry
    let auxv_size = (auxv.len()+1) * std::mem::size_of::<AuxvPair>();
    let auxv_ptr = unsafe {stack_top.add(std::mem::size_of::<AuxvPair>()).sub(auxv_size) as *mut AuxvPair};
    let auxv_slice: &mut [AuxvPair] = unsafe { std::slice::from_raw_parts_mut(auxv_ptr, auxv.len()+1)};

    for i in 0..auxv.len() {
        auxv_slice[i].key = auxv[i].key;
        auxv_slice[i].value = auxv[i].value;
    }
    auxv_slice[auxv.len()] = AuxvPair{key: AT_NULL, value: 0};
    
    let stack = unsafe {
        let mut stack = (auxv_ptr as *mut usize).sub(1);
        // Push envp
        *stack = 0;
        stack = stack.sub(1);
        // Push argv
        // Push the NULL terminator
        *stack = 0;
        stack = stack.sub(1);
        for ptr in argv.iter().rev() {
            *stack = ptr.as_ptr() as usize;
            stack = stack.sub(1);
        }

        // Put argc
        *stack = argv.len();
        stack
    };

    println!("Starting SHELF");

    unsafe {
        asm!(
            "mov rsp, {0}",
            "jmp {1}",
            in(reg) stack,
            in(reg) entry,
            options(noreturn),
        );
    }
}

fn process_elf(elf: &Elf, raw_file: &[u8], argv: &[String], mut auxv: Vec<AuxvPair>) {
    // Setup Argv
    let mut new_argv: Vec<CString> = vec![];
    for arg in argv {
        new_argv.push(CString::new(arg.as_str()).unwrap());
    }


    let mut load_phdr: Option<&goblin::elf::ProgramHeader> = None;
    /*let load_phdr = elf.program_headers.iter().find(|&ph|
        ph.p_type == goblin::elf::program_header::PT_LOAD
    ).expect("No loadable segments");*/
    
    // Get relevant headers
    let mut phdrs: Vec<ProgramHeader> = vec![];
    for h in elf.program_headers.iter() {
        match h.p_type {
            goblin::elf::program_header::PT_LOAD => {
                load_phdr = Some(h)
            },
            goblin::elf::program_header::PT_TLS|goblin::elf::program_header::PT_DYNAMIC =>
                phdrs.push(ProgramHeader::from(h.clone())),
            _ => (),
        }
    }
    let load_phdr = load_phdr.unwrap();

    // Load the loadable segment
    // Should probably verify segment contents
    let load_vaddr: usize = load_phdr.p_vaddr.try_into().unwrap();
    let load_offset: usize = load_phdr.p_offset.try_into().unwrap();
    let load_filesz: usize = load_phdr.p_filesz.try_into().unwrap();
    let load_perms: i32 = load_phdr.p_flags.try_into().unwrap();
    let mem_size: usize = load_phdr.p_memsz.try_into().unwrap();
    let (mapping, entry) = unsafe {
        let mapping = mmap(std::ptr::null_mut(), mem_size+load_vaddr, load_perms,
            MAP_PRIVATE|MAP_ANONYMOUS, -1, 0);
        println!("mapping: {:?}", mapping);
        // Copy the loadable segment
        let src: &[u8] = &raw_file[load_offset..load_offset+load_filesz];
        std::ptr::copy_nonoverlapping(src.as_ptr(), mapping.add(load_vaddr) as *mut u8, src.len());

        (mapping, mapping.add(elf.entry.try_into().unwrap()))
    };
    println!("entry: {:?}", entry);

    // Copy in the phdrs
    let phnum = phdrs.len();
    let phdrs = unsafe {
        let dst_phdrs_ptr = mapping.add(elf.header.e_phoff.try_into().unwrap());
        let dst_phdrs = std::slice::from_raw_parts_mut(dst_phdrs_ptr as *mut ProgramHeader, phnum);
        dst_phdrs.copy_from_slice(&phdrs);
        dst_phdrs_ptr
    };

    // Make the stack
    let stack_phdr = elf.program_headers.iter().find(|&ph|
        ph.p_type == goblin::elf::program_header::PT_GNU_STACK
    ).expect("No stack segment");
    let stack_size = 8 * 1024 * 1024; // 8 MB STACK
    let stack_perms: i32 = stack_phdr.p_flags.try_into().unwrap();
    let stack = unsafe {
        let base_stack = mmap(std::ptr::null_mut(), stack_size, stack_perms,
            MAP_PRIVATE|MAP_ANONYMOUS|MAP_STACK, -1, 0);
        println!("base_stack: {:?}", base_stack);
        base_stack.add(stack_size-16)
    };
    println!("stack: {:?}", stack);
    
    setup_auxv(&mut auxv, phdrs, phnum.try_into().unwrap(), entry);
    exec_shelf(entry, stack, &new_argv, &auxv);
}

fn main() -> error::Result<()> {
    // Save auvx before our programs starts fucking with shit
    let auxv: Vec<AuxvPair> = unsafe{
         auxv::stack::iterate_stack_auxv()
    }.collect();
    println!("auxv: {:?}", auxv);

    let args: Vec<String> = std::env::args().collect();
    let arg = args.get(1).expect("no SHELF given");
    let path = Path::new(&arg);
    let buffer = std::fs::read(path)?;
    match Object::parse(&buffer)? {
        Object::Elf(elf) => {
            process_elf(&elf, &buffer, &args[1..], auxv);
        },
        _ => {
            println!("filetype not supported");
        }
    }
    Ok(())
}
