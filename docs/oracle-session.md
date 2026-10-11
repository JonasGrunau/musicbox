# Oracle session (you, in rekordbox, about 60 minutes)

**Goal:** rekordbox 7.2.19 analyses and exports a known set of audio files, and musicbox is later checked against that output byte by byte. This all happens in the disposable sandbox library. Your stick is not involved.

Before you start, I prepare two things:
- the corpus in `~/re-work/corpus/`
- a read-only snapshot of the current sandbox state

You need a **spare** USB stick or SD card (not your reference stick), erased in Disk Utility as MS-DOS (FAT) with Master Boot Record and named `RBTEST`. Disk images do not work: rekordbox only accepts devices whose protocol is `USB` or `Secure Digital` (docs/re/export.md §6).

## Pass A: Normal mode (about 40 min)
1. **Eject the stick.**
   - In Terminal: `diskutil eject /dev/disk6`
   - rekordbox must never see it.
2. **Plug in the spare `RBTEST` stick.**
3. **Start the instrumented copy of rekordbox from Terminal** (it logs every SQL statement to `~/re-work/traces/`):
   ```
   ~/re-work/rekordbox-instr.app/Contents/MacOS/rekordbox
   ```
   - Leave that Terminal window open.
   - If the copy refuses to start, complains about the app being damaged, or asks you to sign in again, quit it and start the normal rekordbox instead. Tell me which happened.
   - Wait until it is idle. If it asks to update or restore anything, cancel.
4. **Settings → Analysis.** Your UI is German; the labels below are English approximations. Set:
   - Track analysis mode: **Normal** (not Dynamic)
   - BPM range: leave it as it is, and note which value it shows
   - Analyse: **BPM/Grid ON, Key ON, Phrase ON, Vocal ON**
   - High-precision beat analysis: **OFF**
   - Any option that uses analysis results from the internet or cloud: **OFF**. This is `trackAnalysisSettingsNetworkAnalysis`; it must be off, otherwise rekordbox downloads results instead of computing them.
   - Auto gain: leave **ON**
   - Write down anything that is greyed out, for example phrase analysis on a free plan.
5. **Import the corpus.**
   - Drag the folder `~/re-work/corpus/` into the collection.
6. **Create two playlists:**
   - `oracle-all`: every corpus track
   - `oracle-small`: the 5 tracks named in `~/re-work/corpus/oracle-small.txt`
7. **Analyse.**
   - Select all tracks in the collection, then right-click → *Analyse track*.
   - Wait until every track is done.
8. **Export `rekordbox.xml`.**
   - *File → Export Collection in xml format*.
   - Save it as `~/re-work/oracle/rekordbox.xml`.
9. **Export to the spare `RBTEST` stick.**
   - Export `oracle-small` to `RBTEST` first.
   - Then export `oracle-all` to `RBTEST`.
   - Use the default export settings.
10. **Quit rekordbox completely.**
    - Leave `RBTEST` plugged in. I remount it read-only and copy it.
11. **Tell me "pass A done".**
    - I snapshot the library and the image (read-only copies).

## Pass B: high precision + Dynamic (about 20 min, after my snapshot)
1. Start rekordbox. Set **High-precision beat analysis ON** and **Track analysis mode Dynamic**. Leave everything else as in pass A.
2. Re-analyse all tracks in the collection. If rekordbox asks whether to overwrite, choose yes.
3. Quit rekordbox, then tell me "pass B done".

## One-time: make macOS always mount the stick read-only
Run this once in your own Terminal (it asks for your password):
```
echo 'UUID=2335E32F-F224-3E76-8EE8-42C79B582A15 none msdos ro' | sudo tee -a /etc/fstab
```
From then on the stick (volume UUID `2335E32F-…`) always mounts read-only, even when you replug it.

**To undo:** `sudo vifs` and delete that line.

## Afterwards
- While rekordbox is running, never plug in your stick.
