# Grabbit

[English](README.md) · **Italiano**

Scarica **video** e **musica** da YouTube e Spotify con un'interfaccia semplice.
Scritto in Rust (leggero: l'AppImage pesa circa 4 MB), si appoggia a
[yt-dlp](https://github.com/yt-dlp/yt-dlp) e funziona su Windows, Linux e macOS.
L'interfaccia è in italiano se il sistema è in italiano, in inglese per qualsiasi altra lingua.

## Cosa fa

- **Video**: MP4 / MKV / WebM, qualità massima a scelta (fino a 4K).
- **Audio**: MP3, M4A (AAC), Opus, OGG Vorbis, FLAC, WAV, con scelta del **bitrate**.
- Cartella di destinazione a scelta, più link in un colpo solo (uno per riga).
- **Ricerca**: scrivi artista e titolo (o il nome di un video) nella barra *Cerca*, scegli tra i
  risultati di YouTube e premi *Scarica* (subito) o *+* (aggiungi ai link). Si possono
  spuntare più risultati e scaricarli insieme, nel formato scelto (Video o Audio).
  Ogni risultato mostra la copertina (un clic sopra lo seleziona).
  Con *YouTube Music · artista (discografia)* scrivi il nome di un artista e ottieni tutti i
  suoi album e singoli, ciascuno da scaricare per intero.
  Si può cercare su **YouTube** o su **YouTube Music** (brani, album o video): un album si
  scarica tutto, in una cartella con il suo nome. Per usare il proprio account Google,
  scegli il browser in cui hai fatto l'accesso in *Opzioni avanzate → Usa i cookie del browser*.
- **Più download insieme** (di serie 3, da 1 a 4 nelle opzioni avanzate): un album si divide nei
  suoi brani, scaricati in parallelo, con il numero del brano anche nei metadati.
- **Playlist**: i file vanno in una cartella con il nome della playlist e si chiamano
  `1 - Titolo.mp4`, `2 - Titolo.mp4`… (senza il nome della playlist nel nome del file).
- Copertina e metadati incorporati nei file.
- Coda con avanzamento in tempo reale e pulsante **Annulla**.
- **Si prepara da solo**: al primo avvio scarica yt-dlp, ffmpeg e Deno
  (il runtime JavaScript che yt-dlp richiede per YouTube); a ogni avvio
  controlla se esiste un yt-dlp più recente e lo aggiorna.
- **Si aggiorna da solo**: a ogni avvio, subito (senza aspettare il resto), guarda l'ultima
  release GitHub e, se è più recente, la scarica, sostituisce l'app e la riavvia.
  In fondo alla finestra una riga dice sempre com'è andato il controllo.
  Su Linux funziona dall'AppImage; si può disattivare nelle opzioni avanzate.

### Come funziona Spotify

Spotify cifra l'audio (DRM) e yt-dlp non lo scarica. Grabbit legge soltanto i
**dati pubblici** del link (titolo e artista di brano, album o playlist), poi
**cerca lo stesso brano su YouTube** e scarica l'audio da lì. Quindi:

- il risultato è l'audio della versione YouTube, non il file originale Spotify;
- una playlist può mostrare al massimo circa 100 brani;
- se Spotify cambia la struttura delle sue pagine pubbliche, la lettura dei link
  può smettere di funzionare finché non si aggiorna Grabbit.

> Usa Grabbit solo per contenuti che hai il diritto di scaricare e nel rispetto
> del copyright e dei termini di servizio delle piattaforme.

## Installazione

| Sistema | File dalla pagina *Releases* | Note |
|---|---|---|
| Windows | `grabbit-windows-x86_64.exe` | doppio clic; SmartScreen può avvisare perché l'eseguibile non è firmato |
| Linux | `grabbit-linux-x86_64.AppImage` | `chmod +x` e doppio clic (l'auto-aggiornamento funziona solo da AppImage) |
| macOS | `Grabbit-macos-aarch64.app.zip` (Apple Silicon) oppure `…-x86_64.app.zip` (Intel) | decomprimi, trascina in *Applicazioni*; la prima volta tasto destro → Apri (l'app non è notarizzata). Se macOS la dichiara «danneggiata»: `xattr -cr /Applications/Grabbit.app` |

Dati e strumenti scaricati stanno in:
Windows `%APPDATA%\Grabbit\Grabbit\data`, Linux `~/.local/share/grabbit`,
macOS `~/Library/Application Support/org.Grabbit.Grabbit`.

## Creare una release

Le versioni per Windows, Linux e macOS si compilano **su GitHub**, in automatico
(`.github/workflows/release.yml`). Non serve nessun programma installato sul tuo PC.

### Preparazione (una volta sola)

1. Crea un repository su GitHub e caricaci il progetto (compresa la cartella `.github`).
2. Non serve modificare `Cargo.toml`: le versioni compilate da GitHub cercano gli
   aggiornamenti nel repository stesso da cui sono state compilate. (Solo se compili in
   locale, imposta `repository` in `Cargo.toml` con il tuo indirizzo GitHub.)
3. Su GitHub: *Settings → Actions → General → Workflow permissions* e scegli
   **Read and write permissions** (serve per creare tag e release).

### Ogni volta che vuoi pubblicare una versione

1. Su GitHub apri la scheda **Actions** → a sinistra **Release** → **Run workflow**.
2. Nel campo *Nuova versione* scrivi il numero, per esempio `0.2.0`, e conferma.
3. Aspetta 10-20 minuti. Il workflow, da solo:
   - aggiorna la versione in `Cargo.toml`/`Cargo.lock` e crea il tag `v0.2.0`;
   - compila Windows, Linux e macOS (Intel e Apple Silicon);
   - pubblica la release nella pagina **Releases** con tutti i file.
4. Le app già installate trovano la nuova release al prossimo avvio e si aggiornano.

I numeri seguono il formato `maggiore.minore.patch`: `0.1.0` → `0.1.1` per una
correzione, `0.2.0` per una novità, `1.0.0` per la prima versione stabile.
Una versione già usata non si può ripubblicare: scegline una più alta.

**Alternative**

- *Da terminale*: alza `version` in `Cargo.toml`, fai commit, poi
  `git tag v0.2.0 && git push --tags`. Parte lo stesso workflow
  (il tag deve coincidere con la versione di `Cargo.toml`, altrimenti si ferma).
- *Solo prova*: *Run workflow* lasciando **vuoto** il campo versione. Compila tutto
  e lascia i file in *Artifacts* a fondo pagina, senza pubblicare nulla.

### File prodotti (nomi da non cambiare: li cerca l'aggiornamento automatico)

| File | A cosa serve |
|---|---|
| `grabbit-windows-x86_64.exe` | Windows |
| `grabbit-linux-x86_64.AppImage` | Linux Intel/AMD |
| `grabbit-linux-aarch64.AppImage` | Linux ARM (solo se il repository è pubblico) |
| `Grabbit-macos-aarch64.app.zip` / `Grabbit-macos-x86_64.app.zip` | macOS, prima installazione |
| `grabbit-macos-aarch64` / `grabbit-macos-x86_64` | macOS, usati dall'aggiornamento automatico |

### Se qualcosa va storto

- **«Permission denied» o «protected branch»** nel passo *prepare*: i permessi del
  punto 3 della preparazione mancano, oppure il ramo `main` è protetto e non accetta
  commit dal workflow. In quel caso alza la versione a mano e usa il metodo del tag.
- **«Process completed with exit code 126»**: un file di script ha perso il permesso
  di esecuzione nel caricamento (zip/Windows). Il workflow lancia già gli script con
  `bash`, quindi basta usare l'ultima versione dei file.
- **Avvisi «Node.js 20 is deprecated»**: sono solo avvisi di GitHub, non errori.
- **Una build fallisce**: apri il lavoro rosso, leggi l'errore (di solito è nelle
  ultime righe) e correggi il codice. La release non viene pubblicata a metà: esce solo
  se Windows, Linux e macOS riescono. Dopo la correzione, rilancia con una versione nuova
  (se il workflow ha già creato il tag, cancellalo da *Code → Tags* o usa il numero successivo).
- Il workflow **CI** gira a ogni push e controlla formattazione, avvisi e test su
  tutti e tre i sistemi, così gli errori si vedono prima della release.

## Accessibilità

Grabbit rende la sua interfaccia disponibile agli screen reader tramite
[AccessKit](https://github.com/AccessKit/accesskit): Narrator e NVDA su Windows,
VoiceOver su macOS, Orca su Linux. Pulsanti, campi, elenchi e barre di avanzamento hanno nome
e ruolo, il campo del link ha un'etichetta, lo stato dei download è detto anche a parole
(in attesa, in corso, completato, non riuscito) e non solo con il colore, e tutto si
raggiunge da tastiera (Tab / Maiusc+Tab, Spazio o Invio). La dimensione del testo si
aumenta dalle opzioni avanzate. Se trovi qualcosa che uno screen reader non legge bene,
apri una segnalazione.

## Compilare in locale su Windows

Con Rust installato (<https://rustup.rs>) fai doppio clic su `packaging\windows\build.bat`:
crea `grabbit-windows-x86_64.exe`.

## Compilare da sorgente

```bash
cargo run --release                      # avvio
cargo test                               # test unitari
packaging/linux/build-appimage.sh        # crea l'AppImage (Linux)
python3 scripts/make_icon.py             # rigenera le icone (serve Pillow)
```

Serve Rust 1.85+. Su Linux non servono librerie di sviluppo particolari.

Prove con yt-dlp reale (facoltative):

```bash
GRABBIT_YTDLP=/percorso/yt-dlp cargo test -- --ignored
```

Per forzare la lingua dell'interfaccia (prove): `GRABBIT_LANG=it` oppure `GRABBIT_LANG=en`.

## Struttura

| File | Compito |
|---|---|
| `src/app.rs` | interfaccia grafica (egui) |
| `src/downloader.rs` | costruisce i comandi yt-dlp, legge l'avanzamento, annulla |
| `src/tools.rs` | scarica/aggiorna yt-dlp, Deno, ffmpeg |
| `src/spotify.rs` | legge titolo/artista dai link Spotify |
| `src/selfupdate.rs` | aggiornamento automatico dell'app |
| `src/config.rs` | impostazioni salvate |
| `src/i18n.rs` | lingua dell'interfaccia (italiano / inglese) |

## Problemi comuni

- **L'aggiornamento automatico non parte**: leggi la riga di stato in fondo alla finestra.
  Dice se Grabbit è già aggiornato, se non c'è nessuna release (o il repository è privato:
  l'app non può leggerlo, serve un repository pubblico), se GitHub ha limitato le richieste,
  o se la release non contiene il file per il tuo sistema. Il pulsante *Controlla
  aggiornamenti di Grabbit* nelle opzioni avanzate rifà il controllo subito. Dopo un
  aggiornamento l'app si riavvia una volta sola e non si aggiorna di nuovo in quella
  esecuzione, per evitare cicli.

- **«Sign in to confirm you're not a bot» / video con restrizioni**: in
  *Opzioni avanzate* scegli il browser da cui prendere i cookie.
- **macOS**: ffmpeg viene scaricato in versione Intel (su Apple Silicon usa
  Rosetta); se hai Homebrew (`brew install ffmpeg`) viene usato quello.
- **Linux senza AppImage** (binario avviato a mano): funziona, ma l'app non
  può sostituire se stessa; mostra solo il link alla nuova release.

## Licenza

Grabbit è software libero, rilasciato con la
[Licenza Pubblica Generica GNU v3.0 o successive](https://www.gnu.org/licenses/gpl-3.0.html)
(`GPL-3.0-or-later`); vedi `LICENSE`. Puoi usarlo, studiarlo, condividerlo e modificarlo; se
distribuisci versioni modificate, devi rilasciarle con la stessa licenza e con i sorgenti.
