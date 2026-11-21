# Piano: funzionalità di Dropall da portare in Grabbit

*Analisi di Dropall 0.4.3 (repository `marcogiannini/dropall`, sorgente `dropall-0.4.3.zip` e app
`Dropall-macos-x86_64.app.zip`). Grabbit di riferimento: 0.7.0.*

## In breve

Dropall è il progetto da cui Grabbit è nato, rinominato e ampliato dal suo autore. Rispetto a
Grabbit ha **tre aree nuove**: un *download manager* per file qualsiasi, i *torrent* e l'icona
nell'*area di notifica*. Grabbit invece ha tutto ciò che abbiamo costruito noi (ricerca su YouTube
e YouTube Music, discografie, player e libreria, cartelle Artista / Album, niente doppioni…), che
Dropall non ha.

Consiglio di portare, nell'ordine:

1. **Limite di velocità** per tutti i download (piccolo, utile subito).
2. **Download manager** per file da qualsiasi sito, con più connessioni, code e pausa/ripresa
   (grande, ma solo codice Rust: nessun rischio per High Sierra).
3. **Area di notifica** (icona nella barra dei menu del Mac) per scaricare con la finestra
   nascosta (medio).
4. **Torrent** solo dopo una prova dedicata: è proprio la parte che oggi impedisce a Dropall di
   partire sul tuo iMac.

---

## 1. Cosa ho analizzato

| Fonte | Cosa contiene | Cosa ne ho ricavato |
| --- | --- | --- |
| Repository GitHub | README, note di rilascio, workflow | elenco delle funzioni, come si compila e pubblica |
| `dropall-0.4.3.zip` | sorgente completo, 8.030 righe di Rust + shim C++ | come è fatta ogni funzione, cosa si può riusare |
| `Dropall-macos-x86_64.app.zip` | l'app per Mac Intel | perché non parte su High Sierra (sotto) |

### Perché Dropall 0.4.3 non parte sul tuo iMac

- L'eseguibile dichiara di funzionare da **macOS 10.12** (`LC_VERSION_MIN_MACOSX 10.12`, il valore
  di serie di Rust), quindi macOS prova ad avviarlo.
- Ma contiene **libtorrent**, una libreria C++ compilata su `macos-latest` senza
  `MACOSX_DEPLOYMENT_TARGET`. Né `release.yml` né `build.rs` né `native/CMakeLists.txt` lo
  impostano. Così il codice C++ usa `std::bad_function_call::~bad_function_call()`, che la
  `libc++` di High Sierra non ha: all'avvio dyld non lo trova e l'app si chiude
  («Symbol not found … Expected in: /usr/lib/libc++.1.dylib»).
- La parte Rust andrebbe bene: il problema è solo il C++. **Lezione per Grabbit:** qualsiasi
  libreria C/C++ che aggiungiamo va compilata con il macOS minimo esplicito (vedi §6).

## 2. Confronto

| Funzione | Dropall 0.4.3 | Grabbit 0.7.0 |
| --- | --- | --- |
| Video e musica da YouTube (yt-dlp), formati, qualità, copertine | sì | sì |
| Link Spotify (audio cercato su YouTube) | sì | sì |
| Aggiornamento automatico, italiano/inglese, lettore di schermo | sì | sì |
| Ricerca su YouTube e YouTube Music, discografie, scelta dei brani | no | **sì** |
| Player e libreria (gruppi per artista, Casuale/Ripeti, volume uguale…) | no | **sì** |
| Cartelle Artista / Album, niente doppioni, anno di uscita | no | **sì** |
| Barra dei download, azioni sui brani, notifica, link copiato | in parte | **sì** |
| Funziona su High Sierra (iMac 2009) | **no** (0.4.3) | **sì** |
| **Download manager** per file http(s): 1–16 connessioni, code, pianificatore, pausa/ripresa | **sì** | no |
| **Limite di velocità** unico (anche per yt-dlp) | **sì** | no |
| **Torrent** (magnet, `.torrent`, seed, limite di upload) | **sì** | no |
| **Area di notifica**: riduci a icona e continua a scaricare | **sì** | no |
| **Istanza unica**: un link magnet dal browser va all'app già aperta | sì (non su Mac) | no |
| **Trascina e rilascia** file `.torrent` sulla finestra | sì | no |
| Finestra «Licenze e crediti» | sì | no |
| Installer per Windows (Inno Setup) | sì | no (solo `.exe`) |

## 3. Funzionalità candidate, in dettaglio

Per ognuna:
- cosa fa in Dropall;
- quanto ti serve;
- come la porteremmo;
- i rischi per High Sierra;
- la dimensione del lavoro: piccolo, medio o grande.

### 3.1 Limite di velocità per tutti i download

- **In Dropall:** casella «Limita la velocità a» con valori rapidi (256 KB/s … 20 MB/s). Vale per
  il download manager, i torrent e anche yt-dlp, con `--limit-rate 512K`. Per i video già partiti
  vale dal successivo.
- **A cosa ti serve:** mentre Grabbit scarica un album, la rete di casa resta usabile (video,
  chiamate, altri computer).
- **Come:**
  - campo `speed_limit_kbps` nelle impostazioni;
  - l'argomento `--limit-rate` in `build_args` (Dropall ha già il test
    `speed_limit_is_passed_to_ytdlp`);
  - la casella in «Opzioni avanzate» o nella barra dei download.

  Con più download contemporanei il limite va diviso tra i brani in corso, che Dropall non fa
  per yt-dlp. Esempio: con 3 brani insieme e 1 MB/s, ognuno riceve 340 KB/s.
- **High Sierra:** nessun rischio.
- **Lavoro:** piccolo.

### 3.2 Download manager per file da qualsiasi sito

- **In Dropall** (scheda «File», `src/dm.rs` 1.377 righe, `src/app/dm_tab.rs` 707 righe, 16 test):
  - **più connessioni per file** (1–16): richieste `Range`, blocchi da almeno una certa
    dimensione, e chi finisce prima prende il blocco successivo;
  - **code** indipendenti: ognuna scarica i suoi file in ordine, con «Su»/«Giù», pausa, ripresa,
    riprova e rimuovi;
  - **pianificatore** per coda (ora di inizio e di fine, giorni della settimana, anche a cavallo
    della mezzanotte). Funziona solo con l'app aperta;
  - **pausa e ripresa anche dopo la chiusura**: lo stato e i blocchi sono salvati su disco, file
    temporaneo `nome.ID.part`;
  - **limite di velocità condiviso** tra tutte le connessioni (`Limiter`);
  - **prova del server** prima di scaricare: dimensione e supporto di `Range`. Se il server non
    supporta gli intervalli usa una connessione sola e lo scrive.
- **A cosa ti serve:** scaricare file normali (programmi, PDF, archivi, ISO) con lo stesso
  programma, riprendendo i download interrotti. Sull'iMac, con una linea lenta, la ripresa vale
  più della velocità.
- **Come:**
  - nuova scheda **«File»** accanto a «Scarica» e «Ascolta» (vedi la skill `ux-design`);
  - si riusa gran parte di `dm.rs` e dei suoi test, perché la licenza è la stessa (GPL-3.0-or-later):
    in testa al file va l'attribuzione «adattato da Dropall di Marco Giannini»;
  - dipendenze: `ureq`, che Grabbit ha già, più `chrono` per il pianificatore, oppure un calcolo
    dell'ora locale nostro;
  - va integrato con ciò che Grabbit ha già: la barra dei download in basso, la notifica a fine
    download, «Mostra nel Finder» e «Già scaricato».
- **High Sierra:** nessun rischio, è solo Rust e rete.
- **Lavoro:** grande. Lo dividerei in due versioni:
  - prima download singoli con più connessioni e pausa/ripresa;
  - poi code e pianificatore.

### 3.3 Area di notifica (icona nella barra dei menu)

- **In Dropall** (`src/tray.rs`, crate `tray-icon` su Mac e Windows, `ksni` su Linux):
  - minimizzando, la finestra si nasconde nell'icona e i download continuano;
  - lo stesso succede chiudendo la finestra mentre si scarica;
  - il menu dell'icona ha «Mostra» ed «Esci»;
  - l'opzione si spegne in fondo alla finestra.
- **A cosa ti serve:** lasciare Grabbit a scaricare (o a suonare) senza la finestra in mezzo.
- **Come:** si porta `tray.rs` e lo si aggancia alla chiusura della finestra
  (`ViewportCommand::Visible`). Sul Mac l'icona sta nella barra dei menu in alto, e va deciso se
  tenere anche l'icona nel Dock. In più, il menu dell'icona potrebbe avere «Pausa» e «Brano
  successivo» per il player.
- **High Sierra:** da provare. `tray-icon` usa AppKit (`NSStatusItem`, che esiste da sempre), ma
  la versione minima di macOS della crate e delle sue dipendenze `objc2` va controllata sull'iMac.
- **Lavoro:** medio.

### 3.4 Torrent

- **In Dropall** (scheda «Torrent», `src/torrent.rs`, `src/app/torrent_tab.rs`, shim C++
  `native/gt.cpp`, libtorrent 2.0 in `third_party/` da 6,2 MB di sorgenti, Boost e OpenSSL):
  - aggiunge link magnet, indirizzi `.torrent`, file `.torrent` (pulsante, trascinamento,
    «Apri con»);
  - riprende i torrent dopo un riavvio;
  - permette di continuare a condividere (seed), con un limite di upload;
  - rimuove un torrent con o senza i suoi file.
- **A cosa ti serve:** solo se usi BitTorrent per contenuti legali, per esempio distribuzioni
  Linux o archivi pubblici. Ricorda che mentre scarica il computer invia dati ad altri.
- **Come:**
  - libtorrent si compila con CMake dentro `build.rs`, come in Dropall;
  - va dietro a una *feature* di Cargo, così si può compilare anche senza;
  - per le licenze di libtorrent, Boost e OpenSSL serve la finestra «Licenze e crediti» (§3.6).
- **High Sierra:** è il **rischio maggiore** ed è ciò che oggi rompe Dropall. Servono:
  1. `MACOSX_DEPLOYMENT_TARGET=10.13` nel job macOS e `CMAKE_OSX_DEPLOYMENT_TARGET=10.13` per
     CMake, così il compilatore usa le versioni delle funzioni C++ disponibili su 10.13;
  2. la compilazione con `-Werror=unguarded-availability-new`, in modo che ogni funzione C++
     troppo nuova blocchi la build invece di rompere l'app all'avvio;
  3. la prova sul tuo iMac prima di pubblicare.

  libtorrent potrebbe usare funzioni introdotte dopo 10.13, come `std::filesystem` (10.15) o la
  memoria allineata (10.14). In quel caso le strade sono due: compilare la versione per Mac Intel
  senza torrent, oppure collegare staticamente una `libc++` recente.
- **Altri costi:**
  - l'app passerebbe da ~3,5 MB a ~7 MB;
  - la compilazione su GitHub si allungherebbe di diversi minuti;
  - va gestita la porta 6881, con UPnP/NAT-PMP.
- **Lavoro:** grande. Prima una **prova tecnica** (solo compilare per 10.13 e aprire l'app
  sull'iMac); la scheda completa solo se la prova riesce.

### 3.5 Istanza unica e link dal browser

- **In Dropall** (`src/ipc.rs`): se Dropall è già aperto, un secondo avvio (per esempio da un link
  magnet nel browser) passa il link all'istanza aperta, attraverso una connessione locale protetta
  da un token, e si chiude.
- **Sul Mac:** i link e i file aperti con un'app arrivano come *Apple Events*, non come argomenti.
  Dropall non li gestisce sul Mac: lo dice il suo README.
- **Per Grabbit:** utile solo insieme ai torrent, oppure per aprire con Grabbit i link YouTube da
  un'app. Sul Mac serve gestire l'evento `kAEGetURL` (objc) e registrare gli schemi nell'`Info.plist`.
- **Lavoro:** medio, sul Mac. **Priorità bassa.**

### 3.6 Licenze e crediti, Novità

- **In Dropall:** finestra «Licenze e crediti» con il testo di GPL e delle librerie
  (`THIRD_PARTY_LICENSES.txt`, incluso anche nel pacchetto); pannello «Novità» dalle note di
  rilascio.
- **Per Grabbit:** il pannello «Novità» lo abbiamo già (nell'avviso di aggiornamento). «Licenze e
  crediti» diventa **obbligatoria** se includiamo librerie con licenza da citare (libtorrent,
  Boost, OpenSSL), ed è comunque buona pratica per yt-dlp, ffmpeg e Deno, che Grabbit scarica.
- **Lavoro:** piccolo.

### 3.7 Altro, priorità bassa

- **Installer per Windows** (Inno Setup, senza diritti di amministratore): utile solo se qualcuno
  usa Grabbit su Windows.
- **Trascina e rilascia:** in Dropall solo per i file `.torrent`. Per i link trascinati dal
  browser vale quanto detto per la 0.5.0: sul Mac non è affidabile, e l'alternativa del link
  copiato c'è già.
- **Una riga di spiegazione sotto ogni scheda** («Scarica con BitTorrent da link magnet…»): se
  aggiungiamo schede, conviene adottarla.

## 4. Cosa non portare

- La scheda «Video e musica» di Dropall: quella di Grabbit fa molto di più.
- Il tema scuro forzato e i testi di Dropall: Grabbit ha già i suoi, verificati sul Mac.
- La configurazione di rilascio di Dropall così com'è: senza `MACOSX_DEPLOYMENT_TARGET` romperebbe
  il supporto a High Sierra.

## 5. Piano a versioni

Ogni versione segue il nostro giro:
1. storie in `docs/STORIES.md` con le prove;
2. codice, test e screenshot;
3. la tua prova sull'iMac;
4. pull request, merge e release.

| Versione | Contenuto | Lavoro | Rischio su High Sierra |
| --- | --- | --- | --- |
| **0.8.0** | Limite di velocità (§3.1) e **download manager, parte 1**: scheda «File», un file alla volta con più connessioni, pausa/ripresa anche dopo la chiusura, barra di avanzamento con percentuale, «Mostra nel Finder» | medio-grande | nessuno |
| **0.9.0** | **Download manager, parte 2**: più code, «Su»/«Giù», pianificatore per coda, limite condiviso | medio | nessuno |
| **0.10.0** | **Area di notifica** (§3.3) e «Licenze e crediti» (§3.6) | medio | da provare (`tray-icon`) |
| prova tecnica | **Torrent**: solo compilare libtorrent per 10.13 con i controlli del §6 e aprire l'app sull'iMac | medio | **alto** |
| **0.11.0** (se la prova riesce) | Scheda **Torrent** (§3.4) dietro una feature, ed eventualmente l'istanza unica (§3.5) | grande | alto |

## 6. Regole per non perdere High Sierra

1. **Mai codice C/C++ senza macOS minimo:** nel job macOS del workflow di release
   `MACOSX_DEPLOYMENT_TARGET: "10.13"`, e per CMake `CMAKE_OSX_DEPLOYMENT_TARGET`.
2. **Controllo nella CI:** un passo che legge il macOS minimo dichiarato dall'eseguibile e le
   librerie di sistema che importa. Se compare un simbolo noto come assente su 10.13, la build si
   ferma.
3. **Ogni versione si prova sull'iMac** prima della pull request, come facciamo già.
4. **Le crate nuove con parti native** (`tray-icon`, librerie C) si provano prima in una build di
   prova del branch.

## 7. Decisioni che servono da te

1. **Download manager:** ti serve scaricare anche file normali (programmi, archivi, PDF)?
   Se sì, è il punto di partenza consigliato.
2. **Torrent:** li useresti? Se no, li lasciamo fuori: è la parte più costosa e più rischiosa per
   l'iMac.
3. **Area di notifica:** preferisci l'icona nella barra dei menu (in alto) con la finestra
   nascosta, o ti basta il Dock?

## 8. Licenza

Dropall e Grabbit sono entrambi **GPL-3.0-or-later**: il codice di Dropall si può riusare in
Grabbit, purché ogni file adattato riporti la provenienza (Dropall, Marco Giannini) e Grabbit
resti GPL. Le librerie dei torrent hanno licenze compatibili da citare: libtorrent BSD-3-Clause,
Boost BSL-1.0, OpenSSL Apache-2.0.
