# Op: export the collection as rekordbox.xml

Status: **verified** (no DB writes). The XML file (`fixtures/oracle-a/rekordbox.xml`) has mtime 22:07:01.

## master.db
Reads only, all on one worker thread at 22:07:01:
```
select * from djmdMixerParam where rb_local_deleted = 0 and ContentID = <CID>                 (once per track, 161×)
select ContentID, rb_local_deleted from djmdSongPlaylist where PlaylistID = <P> order by TrackNo   (once per playlist, 3×)
```
Everything else (track fields, cues, playlists tree) comes from rekordbox's in-memory cache. No INSERT/UPDATE/DELETE, no transaction, `localUpdateCount` unchanged.

## Notes
- The per-track `djmdMixerParam` read suggests the XML uses the auto-gain values (*open*: which XML attribute).
- The XML format itself is a separate spec.
