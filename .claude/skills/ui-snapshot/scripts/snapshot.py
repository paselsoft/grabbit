#!/usr/bin/env python3
"""Avvia Grabbit su uno schermo virtuale (Xvfb), esegue clic e tasti, salva screenshot.

Grabbit non scarica nulla: yt-dlp, Deno e ffmpeg sono script finti (un «download» dura 10 s), la configurazione e i dati
stanno in una cartella temporanea (la tua configurazione vera non viene toccata), l'audio va su
un'uscita ALSA muta (che suona molto più veloce del tempo reale).

Uso (dalla cartella del repository, dopo `cargo build`):

    python3 .claude/skills/ui-snapshot/scripts/snapshot.py --out /percorso/screenshot \\
        [--music CARTELLA] [--lang it|en] [--app target/debug/grabbit] PASSI...

Passi, eseguiti in ordine:
    shot:NOME      salva NOME.png (tutto lo schermo 1000x1000; la finestra è in alto a sinistra)
    click:X,Y      sposta il mouse, aspetta, preme e rilascia il tasto sinistro
    clip:TESTO     mette TESTO negli appunti (serve xclip)
    scroll:N       N scatti di rotellina giù dove si trova il mouse (negativo: su)
    wait:SECONDI   aspetta
    key:TASTO      un tasto per xdotool (Return, Escape, ctrl+v, ...)
    type:TESTO     scrive del testo nel campo che ha il focus
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

FAKE_TOOLS = {
    # --version risponde subito; un «download» dura 10 s con l'avanzamento che legge Grabbit
    "yt-dlp": """#!/bin/sh
case " $* " in
  *" --version "*) echo 2026.10.26; exit 0;;
  *" --flat-playlist "*)
    # risultati di ricerca preparati (--search FILE): una riga JSON per risultato
    case "$*" in *ytsearch*|*music.youtube.com/search*)
      [ -n "$GRABBIT_FAKE_SEARCH" ] && cat "$GRABBIT_FAKE_SEARCH";;
    esac
    exit 0;;
esac
i=0
while [ $i -le 20 ]; do
  echo "GRB|$((i * 5))|100|NA|1048576|$((20 - i))|NA|NA|Brano di prova"
  i=$((i + 1)); sleep 0.5
done
echo "GRBFILE|/tmp/Brano di prova.m4a"
""",
    "deno": "#!/bin/sh\necho deno 2.5.0\n",
    "ffmpeg": "#!/bin/sh\necho ffmpeg\n",
}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", required=True, help="cartella dove salvare gli screenshot")
    ap.add_argument("--music", help="cartella della musica per la scheda Ascolta")
    ap.add_argument("--lang", default="it", choices=["it", "en"])
    ap.add_argument("--app", default="target/debug/grabbit")
    ap.add_argument("--display", default=":77")
    ap.add_argument("--home", help="cartella della configurazione da tenere tra due avvii "
                    "(per provare la ripresa); di serie una temporanea, cancellata alla fine")
    ap.add_argument("--search", help="file con i risultati che il finto yt-dlp dà alla ricerca "
                    "(una riga JSON per risultato, come --print di yt-dlp)")
    ap.add_argument("steps", nargs="*")
    a = ap.parse_args()

    for tool in ("Xvfb", "xdotool", "import"):
        if not shutil.which(tool):
            print(f"manca {tool}: vedi i requisiti in SKILL.md", file=sys.stderr)
            return 2
    app = os.path.abspath(a.app)
    if not os.path.isfile(app):
        print(f"non trovo {app}: esegui prima `cargo build`", file=sys.stderr)
        return 2
    out = os.path.abspath(a.out)
    os.makedirs(out, exist_ok=True)

    keep = bool(a.home)
    home = os.path.abspath(a.home) if keep else tempfile.mkdtemp(prefix="grabbit-snap-")
    data = os.path.join(home, ".local", "share")
    conf = os.path.join(home, ".config")
    bin_dir = os.path.join(data, "grabbit", "bin")
    os.makedirs(bin_dir, exist_ok=True)
    os.makedirs(os.path.join(conf, "grabbit"), exist_ok=True)
    for name, body in FAKE_TOOLS.items():
        p = os.path.join(bin_dir, name)
        with open(p, "w") as f:
            f.write(body)
        os.chmod(p, 0o755)
    with open(os.path.join(home, ".asoundrc"), "w") as f:
        f.write("pcm.!default { type null }\nctl.!default { type hw card 0 }\n")
    downloads = os.path.join(home, "Download")
    os.makedirs(downloads, exist_ok=True)
    cfg = {"out_dir": downloads, "auto_update_app": False}
    if a.music:
        cfg["player_dir"] = os.path.abspath(a.music)
    cfg_path = os.path.join(conf, "grabbit", "config.json")
    # con --home la configurazione salvata dal primo avvio resta (ultimo brano, opzioni)
    if not (keep and os.path.exists(cfg_path)):
        with open(cfg_path, "w") as f:
            json.dump(cfg, f)

    env = dict(os.environ)
    env.update(
        DISPLAY=a.display,
        HOME=home,
        XDG_DATA_HOME=data,
        XDG_CONFIG_HOME=conf,
        GRABBIT_LANG=a.lang,
        LIBGL_ALWAYS_SOFTWARE="1",
        WINIT_UNIX_BACKEND="x11",
        WAYLAND_DISPLAY="",
    )
    if a.search:
        env["GRABBIT_FAKE_SEARCH"] = os.path.abspath(a.search)

    def x(*cmd):
        subprocess.run(cmd, env=env, check=True)

    def focus():
        # senza window manager nessuno dà il focus della tastiera alla finestra:
        # va dato a mano, altrimenti i tasti si perdono
        subprocess.run(["xdotool", "search", "--name", "Grabbit", "windowfocus"], env=env,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    xvfb = subprocess.Popen(["Xvfb", a.display, "-screen", "0", "1000x1000x24"], stderr=subprocess.DEVNULL)
    time.sleep(1.5)
    log_path = os.path.join(out, "app.log")
    log = open(log_path, "w")
    proc = subprocess.Popen([app], env=env, stdout=log, stderr=log)
    time.sleep(5)
    try:
        for step in a.steps:
            kind, _, arg = step.partition(":")
            if proc.poll() is not None:
                print(f"Grabbit si è chiuso (codice {proc.returncode}): vedi {log_path}", file=sys.stderr)
                return 1
            if kind == "shot":
                x("import", "-window", "root", os.path.join(out, f"{arg}.png"))
                print(os.path.join(out, f"{arg}.png"))
            elif kind == "click":
                cx, cy = arg.split(",")
                x("xdotool", "mousemove", cx, cy)
                time.sleep(0.4)  # egui deve vedere il mouse prima del clic
                x("xdotool", "mousedown", "1")
                time.sleep(0.15)
                x("xdotool", "mouseup", "1")
                time.sleep(0.8)
            elif kind == "clip":
                # mette TESTO negli appunti (serve xclip); il focus che torna alla finestra con
                # il passo successivo (key:, type:) è come tornare su Grabbit dal browser
                subprocess.run(["xclip", "-selection", "clipboard"], input=arg.encode(), env=env, check=True)
                time.sleep(0.3)
            elif kind == "scroll":
                # rotellina del mouse dove si trova il puntatore: N scatti giù (negativo: su)
                n = int(arg)
                x("xdotool", "click", "--repeat", str(abs(n)), "--delay", "50", "5" if n > 0 else "4")
                time.sleep(0.6)
            elif kind == "wait":
                time.sleep(float(arg))
            elif kind == "key":
                focus()
                x("xdotool", "key", arg)
                time.sleep(0.5)
            elif kind == "type":
                focus()
                x("xdotool", "type", "--delay", "40", arg)
                time.sleep(0.5)
            else:
                print(f"passo sconosciuto: {step}", file=sys.stderr)
                return 2
    finally:
        proc.terminate()
        time.sleep(0.5)
        proc.kill()
        xvfb.terminate()
        log.close()
        if not keep:
            shutil.rmtree(home, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
