# Draft enquiry to the `codebreaker` / cb2util author

Status: **draft, not sent.** Needs the owner's sign-off before anything is
posted or emailed. Nothing in this note is a claim of legal clearance: the
crate and `cb2util` are published under MIT (and Apache-2.0 for the crate), and
EmuWiz relies on those licences as written. This enquiry exists to learn what
the licences cannot tell us.

Suggested channel: a GitHub issue or discussion on `mlafeldt/codebreaker-rs`
(public, so the answer helps other users), or email if the author prefers.

## Draft

Subject: codebreaker crate: algorithm origins and a V7 re-key difference from cb2util v1.9

> Hi Mathias,
>
> I maintain EmuWiz, an MIT-licensed tool for managing retro game libraries.
> I'm adding a small preview-only feature that decrypts pasted PS2 CodeBreaker
> codes with your `codebreaker` crate and shows the equivalent PCSX2 PNACH
> lines for direct memory writes. Thank you for publishing it under MIT/Apache.
>
> Three questions, answer whichever you can:
>
> 1. **Origins.** The README says the cipher was reverse-engineered from the
>    R5900 code in 2006. Is there anything we should know about where the V1
>    seed tables and the V7 construction (RC4/SHA-1/multiplicative steps) came
>    from, for example other people's earlier work, or permissions you
>    obtained, that we should credit or be aware of?
> 2. **Independent references.** Do you know of a public specification or test
>    vectors from anyone other than you (documentation, another independent
>    implementation, captured device output)? I could only find your own
>    vectors, so our validation is "agrees with your original C code".
> 3. **A V7 difference.** While checking the crate against cb2util v1.9 I found
>    that they disagree for a common-key stream that contains a `BEEFC0DE`
>    line: the key line itself decrypts alike, but every later line differs.
>    The crate's own test uses `E8C99F89 14B426D6` for the line after
>    `8787C575 1AC4C1B4`, while v1.9 `cb_encrypt_code` produces
>    `02210430 184C16E8` for the same plaintext. Streams with a second
>    `BEEFC0DE` or any `BEEFC0DF` also differ. Ordinary V1, common-key and
>    leading-`B4336FA9 4DFEFB79` lists agree on about 130,000 random lines.
>    Is the difference intended (later cb2util/ps2rd behaviour, or a device
>    quirk), or a bug on one side? A reproducible harness is in
>    `docs/research/CODEBREAKER_RS_C_DISCREPANCY.md` in our repository.
>
> For now EmuWiz refuses any list that re-keys, so nothing depends on the
> answer. I'm happy to open a proper issue with the harness, or to send a PR
> if you tell me which behaviour is right.
>
> Is there a credit line you'd like in our third-party notices beyond the
> copyright and licence text?
>
> Thanks,
> (owner name)

## Handling rules

- Do not paraphrase the answer as "approved" or "cleared"; quote it.
- If the author names earlier contributors, record them in
  `docs/THIRD_PARTY_NOTICES.md` and in the research note.
- A reply cannot widen EmuWiz's accepted shapes by itself: any new accepted
  sequence needs its own validated vectors and independent correctness review.
