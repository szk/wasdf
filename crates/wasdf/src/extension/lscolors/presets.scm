;; Built-in ls-colors palette presets. (palette <name> (raw "key=SGR:key=SGR:..."))
;; `raw` is the same `LS_COLORS`-format string `parse::parse_ls_colors` reads
;; from the environment.
(quote (
  (palette "dark" (raw "di=01;34:ln=01;36:ex=01;32:*.tar=01;31:*.tgz=01;31:*.zip=01;31:*.gz=01;31:*.bz2=01;31:*.xz=01;31:*.7z=01;31:*.rar=01;31:*.jpg=01;35:*.jpeg=01;35:*.png=01;35:*.gif=01;35:*.bmp=01;35:*.svg=01;35:*.mp3=00;36:*.wav=00;36:*.flac=00;36:*.mp4=01;35:*.mkv=01;35:*.avi=01;35:*.mov=01;35:*.pdf=00;31:*.md=00;33:*.rs=00;33:*.py=00;33:*.js=00;33:*.ts=00;33:*.json=00;32:*.toml=00;32:*.yaml=00;32:*.yml=00;32"))
  (palette "light" (raw "di=01;34:ln=01;36:ex=01;32:*.tar=01;31:*.tgz=01;31:*.zip=01;31:*.gz=01;31:*.bz2=01;31:*.xz=01;31:*.7z=01;31:*.rar=01;31:*.jpg=01;35:*.jpeg=01;35:*.png=01;35:*.gif=01;35:*.bmp=01;35:*.svg=01;35:*.mp3=00;36:*.wav=00;36:*.flac=00;36:*.mp4=01;35:*.mkv=01;35:*.avi=01;35:*.mov=01;35:*.pdf=00;31:*.md=00;34:*.rs=00;34:*.py=00;34:*.js=00;34:*.ts=00;34:*.json=00;32:*.toml=00;32:*.yaml=00;32:*.yml=00;32"))))
