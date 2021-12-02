# SHELF Loader PoC

# Usage

## 1. Build a SHELF
What is a SHELF? Read here: https://tmpout.sh/1/10/

First, pick a project you can compile statically. I will use https://github.com/joseluisq/static-web-server. For non-Rust projects the important part of building a SHELF binary is the link args. (Ex. `musl-gcc -static-pie -fPIC -Xlinker -N -fuse-ld=lld  main.c`)

```
$ git clone https://github.com/joseluisq/static-web-server.git
```

Next, we have to make sure it compiles statically, PIE and in one RWX segment.

```
$ cd static-web-server
$ mkdir .cargo
$ echo -e '[build]\ntarget = "x86_64-unknown-linux-musl"\nrustflags = ["-C", "link-args=-static-pie", "-C", "link-args=-N", "-C", "link-args=-fuse-ld=lld"]' > .cargo/config
$ cat .cargo/config
[build]
target = "x86_64-unknown-linux-musl"
rustflags = ["-C", "link-args=-static-pie", "-C", "link-args=-N", "-C", "link-args=-fuse-ld=lld"]
```

We use the musl target as those are only static builds. We also use lld (llvm's linker) as it supports -static-pie with -N so you might need to install if you don't have it.

Now, build the SHELF (and run it to verify it works)
```
$ cargo run --release
```

Verify it is a SHELF
```
$ readelf -l target/x86_64-unknown-linux-musl/release/static-web-server

Elf file type is DYN (Shared object file)
Entry point 0x1b9640
There are 6 program headers, starting at offset 64

Program Headers:
  Type           Offset             VirtAddr           PhysAddr
                 FileSiz            MemSiz              Flags  Align
  LOAD           0x0000000000000190 0x0000000000000190 0x0000000000000190
                 0x00000000004e39d0 0x0000000000534bbc  RWE    0x1000
  TLS            0x00000000004a32b8 0x00000000004a32b8 0x00000000004a32b8
                 0x0000000000001878 0x0000000000001a70  R      0x10
  DYNAMIC        0x00000000004e3388 0x00000000004e3388 0x00000000004e3388
                 0x0000000000000130 0x0000000000000130  RW     0x8
  GNU_EH_FRAME   0x000000000017fb90 0x000000000017fb90 0x000000000017fb90
                 0x0000000000007394 0x0000000000007394  R      0x4
  GNU_STACK      0x0000000000000000 0x0000000000000000 0x0000000000000000
                 0x0000000000000000 0x0000000000000000  RW     0x0
  NOTE           0x0000000000000190 0x0000000000000190 0x0000000000000190
                 0x0000000000000018 0x0000000000000018  R      0x4

 Section to Segment mapping:
  Segment Sections...
   00     .note.gnu.build-id .dynsym .gnu.hash .dynstr .rela.dyn .rodata .eh_frame_hdr .eh_frame .text .init .fini .tdata .fini_array .init_array .data .data.rel.ro .dynamic .got .got.plt .bss 
   01     .tdata .tbss 
   02     .dynamic 
   03     .eh_frame_hdr 
   04     
   05     .note.gnu.build-id 
```

Note that there is one `LOAD` segment, no `INTERP` segment and all the `VirtAddrs` are low.

## 2. Run the SHELF

Clone this repo.
```
$ git clone https://github.com/PinkNoize/shelf-loader-poc.git
```

Run the SHELF
```
$ cargo run --release -- ~/static-web-server/target/x86_64-unknown-linux-musl/release/static-web-server -a 127.0.0.1 --port 6969 --root <Dir to host>
$ curl 127.0.0.1:6969/<File in DIR>
```