# `codebreaker` 0.4.0 vs cb2util 1.9: V7 re-key disagreement

Status: **open upstream investigation** (not yet reported upstream).
Found 2026-10-25 while validating the crate for `ps2_codebreaker`
([PS2_CODEBREAKER_PNACH_PREVIEW.md](PS2_CODEBREAKER_PNACH_PREVIEW.md)).
EmuWiz works around it by refusing the affected sequences; it does not modify
the crate. This note records what was observed, not which implementation is
correct. Which side is right for real devices is unknown.

## Observation

Reference: `mlafeldt/cb2util` tag `v1.9` (`5fed07b`, MIT) `cb2_crypto.c`, with
`mlafeldt/libbig_int` `fd8f327`. Crate: `codebreaker = "=0.4.0"` (crates.io
checksum `88e7e1d6cfc999dfad058f198b13fd539bfa0fefcf7589f976a0c4accf551d7e`).
Streams were encrypted with the C `cb_encrypt_code` and decrypted by the C
`cb_decrypt_code` (round trip always restores the plaintext) and by the crate's
`Codebreaker::decrypt_code`.

| Setup | Result |
| --- | --- |
| C `cb_reset()`, no `BEEFC0DE`/`DF` (V1 per line) | crate agrees (132,023 lines) |
| C `cb_set_common_v7()`, no `BEEFC0DE`/`DF` | crate (`new_v7()`) agrees (132,023 lines) |
| C `cb_reset()`, first line `BEEFC0DE`, then V7 | crate (`new()`) agrees (133,491 lines, random seeds) |
| C `cb_set_common_v7()`, then a `BEEFC0DE` line | **crate disagrees on every line after the key line** |
| a second `BEEFC0DE` mid-stream, or any `BEEFC0DF` | crate disagrees in most streams |

## Smallest reproduction

Plaintext (what the C code encrypts) and ciphertext (C output), common key:

```
BEEFC0DE 00000000 -> 8787C575 1AC4C1B4
9029BEAC 0C0A9225 -> 02210430 184C16E8
201F6024 00000000 -> 32E2A916 7E6017BA
2096F5B8 000000BE -> CBB720FD D61505E0
```

C: `cb_set_common_v7(); cb_encrypt_code(...)` per line, or `cb_decrypt_code` on
the ciphertext, restores the plaintext. Rust:

```rust
let mut cb = codebreaker::Codebreaker::new_v7();
assert_eq!(cb.decrypt_code(0x8787C575, 0x1AC4C1B4), (0xBEEFC0DE, 0)); // ok
assert_ne!(cb.decrypt_code(0x02210430, 0x184C16E8), (0x9029BEAC, 0x0C0A9225)); // differs
```

The crate's own test case with the same first line
(`8787C575 1AC4C1B4` -> `BEEFC0DE 00000000`, then `E8C99F89 14B426D6` ->
`9029BEAC 0C0A9225`) uses different ciphertext for the second line than
cb2util v1.9 produces (`02210430 184C16E8`). So the two disagree already on
the encrypt side for this shape, and the crate's expected output is not what
v1.9 produces.

Candidate explanations to check (none confirmed): v1.9 treats the common-key
state as already initialised (`v7_init = 1`) and re-keys with
`cb7_beefcode(0, val)`, while the crate's `Cb7::default()` followed by a
`BEEFC0DE` line takes a different path; the crate may follow a later cb2util
or ps2rd revision than v1.9; or either may differ from real devices.

## What EmuWiz does

`ps2_codebreaker` accepts only the three agreeing shapes. It refuses any
`BEEFC0DE`/`BEEFC0DF` that is not the single leading V1-encrypted setup line,
and any `BEEFC0DE` under the common key. Regression vectors for each shape are
in `ps2_codebreaker/tests.rs`. The rejection tests substitute scripted
ciphers (the C original's output, a hypothetical fixed crate, and arbitrary
output) and require the same refusal each time, so the policy does not depend
on what any version of the crate returns after a key line. The only crate
behaviour it relies on is that a `BEEFC0DE` key line decrypts to `BEEFC0DE`,
which the crate and the C code agreed on in every tested stream. This note is
the record of the disagreement; no test asserts that it persists.

## Suggested upstream work

1. Run the harness below against cb2util `master` and ps2rd's copy of the
   cipher to see whether the C side changed after v1.9.
2. Find a vector from a real device or an independent tool for the
   common-key-plus-`BEEFC0DE` shape.
3. Ask the author which behaviour is intended; see the enquiry draft in the
   research notes.

## Reproduction harness

Build the C generator inside a cb2util v1.9 checkout (plus `libbig_int`):
`gcc -O1 -w -DCODE_ENCRYPTION -Ilibbig_int/include -I. gen.c cb2_crypto.c
arcfour.c shs.c libbig_int/src/*.c libbig_int/src/low_level_funcs/*.c -o gen`.
Run `./gen <mode> <streams> <profile> | cbcheck`, where mode `0` is `cb_reset`,
mode `1` is `cb_set_common_v7`, and profile `0` = no key lines, `1` = a leading
`BEEFC0DE` only, `2` = random key lines. `ZV=1` forces a zero seed.

`gen.c`:

```c
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include "cb2_crypto.h"
static uint64_t s=0x9E3779B97F4A7C15ULL;
static uint32_t rnd(void){ s^=s<<13; s^=s>>7; s^=s<<17; return (uint32_t)(s>>16); }
/* output: "<mode> <n>" then n lines "plainA plainV encA encV" */
int main(int argc,char**argv){
  int mode=atoi(argv[1]); /* 0=v1 stream (reset), 1=common v7 */
  int streams=atoi(argv[2]); int prof=argc>3?atoi(argv[3]):2;
  for(int t=0;t<streams;t++){
    int n=4+rnd()%60; 
    printf("%d %d\n",mode,n);
    /* encrypt pass */
    uint32_t pa[80],pv[80],ea[80],ev[80];
    for(int i=0;i<n;i++){
      pa[i]=rnd(); pv[i]=rnd();
      uint32_t r=rnd()%12;
      if(prof==0 && (r==0||r==1)) r=5;
      if(prof==1){ if(r<2) r=5; if(i==0){ pa[i]=0xBEEFC0DE; pv[i]=getenv("ZV")?0:((rnd()%2)?0:rnd()); continue; } }
      if(r==0){pa[i]=0xBEEFC0DE; if(rnd()%4==0)pv[i]=0;}
      else if(r==1 && i>0){pa[i]=0xBEEFC0DF;}
      else if(r<8){pa[i]=(rnd()%3<<28)|(pa[i]&0x01FFFFFF);} /* 0/1/2 direct */
    }
    if(mode==1) cb_set_common_v7(); else cb_reset();
    for(int i=0;i<n;i++){ ea[i]=pa[i]; ev[i]=pv[i]; cb_encrypt_code(&ea[i],&ev[i]); }
    for(int i=0;i<n;i++) printf("%08X %08X %08X %08X\n",pa[i],pv[i],ea[i],ev[i]);
    /* C decrypt pass for check */
    if(mode==1) cb_set_common_v7(); else cb_reset();
    for(int i=0;i<n;i++){ uint32_t a=ea[i],v=ev[i]; cb_decrypt_code(&a,&v); printf("C %08X %08X\n",a,v); }
  }
}
```

`cbcheck/src/main.rs` (crate `codebreaker = "=0.4.0"`, edition 2024):

```rust
use codebreaker::Codebreaker;
use std::io::Read;
fn h(s:&str)->u32{u32::from_str_radix(s,16).unwrap()}
fn main(){
  let mut t=String::new(); std::io::stdin().read_to_string(&mut t).unwrap();
  let mut classes=std::collections::BTreeMap::new(); let mut it=t.lines(); let (mut streams,mut lines,mut bad)=(0,0,0);
  while let Some(hd)=it.next(){
    let p:Vec<&str>=hd.split(' ').collect(); let mode:u32=p[0].parse().unwrap(); let n:usize=p[1].parse().unwrap();
    let mut rows=vec![]; for _ in 0..n{ let f:Vec<u32>=it.next().unwrap().split(' ').map(h).collect(); rows.push(f);}
    let mut crow=vec![]; for _ in 0..n{ let l=it.next().unwrap(); let f:Vec<u32>=l[2..].split(' ').map(h).collect(); crow.push((f[0],f[1]));}
    let mut feat=(0u32,0u32); let mut first:Option<usize>=None; let mut cb= if mode==1 {Codebreaker::new_v7()} else {Codebreaker::new()};
    streams+=1;
    for (i,r) in rows.iter().enumerate(){
      let d=cb.decrypt_code(r[2],r[3]); lines+=1; if first.is_none() && (d!=(r[0],r[1])||d!=crow[i]) {first=Some(i);} if r[0]==0xBEEFC0DE {feat.0+=1;} if r[0]==0xBEEFC0DF {feat.1+=1;}
      if d!=(r[0],r[1]) || d!=crow[i] { bad+=1; if bad<6 {eprintln!("MISMATCH mode{mode} line{i}: plain {:08X} {:08X} rust {:08X} {:08X} c {:08X} {:08X}",r[0],r[1],d.0,d.1,crow[i].0,crow[i].1);} }
    }
    let key=(mode,feat.0.min(2),feat.1.min(1),first.is_some()); *classes.entry(key).or_insert(0u32)+=1;
  }
  for (k,v) in &classes {println!("mode{} DE={} DF={} mismatch={} : {} streams",k.0,k.1,k.2,k.3,v);}
  println!("streams={streams} lines={lines} mismatches={bad}");
}
```
