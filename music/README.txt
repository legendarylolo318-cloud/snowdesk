Drop your own music files in this folder.

snowdesk plays mp3 (required), ogg, flac, wav and m4a/aac files. No music is
bundled with the app and none should ever be committed to the repository.

The folder the installed app reads is:
  Linux:   $XDG_DATA_HOME/snowdesk/music   (default ~/.local/share/snowdesk/music)
  Windows: %APPDATA%\snowdesk\music
A `music/` folder next to the executable (or next to the AppImage) is read too.
New files show up in the player automatically.
