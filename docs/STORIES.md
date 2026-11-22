# Storie utente di Grabbit

Una azione per storia. Ogni riga di accettazione dice come si verifica:
`[unit]` test in `cargo test`, `[ci]` CI di GitHub, `[snap]` screenshot con la
skill `ui-snapshot`, `[manual]` da provare sul computer vero (rete, YouTube, Mac).

## Milestone 1: download più chiari e ordinati (0.3.0)

S-001, S-002, S-003

### S-001 Vedere «Scarica» e l'avanzamento senza scorrere
Come utente, nella scheda Scarica trovo sempre in basso una barra con «Scarica»,
il numero di link pronti e, durante il download, l'avanzamento totale con «Annulla».
Accettazione:
- [snap] Con la finestra di serie (780×780) la barra è visibile senza scorrere, con «Scarica» e «Nessun link» o «N link».
- [snap] Durante un download la barra mostra «Annulla», una barra di avanzamento e «X di N completati · 45%»; ogni brano in corso mostra la sua percentuale.
- [snap] «Vedi elenco» porta l'elenco dei download in vista; la barra non compare nella scheda Ascolta.
- [ci] cargo clippy e cargo test passano su Linux, Windows e macOS.
UX:
- Flusso: scegli un risultato o incolla un link -> la barra dice «1 link» -> «Scarica» -> la barra mostra l'avanzamento -> «3 di 3 completati»
- Posto: barra fissa in basso (deve restare visibile mentre si lavora sopra), sopra quella del player
- Stati: vuoto «Nessun link: cerca o incolla un link»; in corso «X di N completati · Y non riusciti»; fatto «N completati» in verde
- Termini nuovi: Barra dei download (Download bar)
Stato: fatta

### S-002 Saltare i brani già scaricati
Come utente, se scarico di nuovo un brano o un album che ho già, Grabbit non lo
riscarica e nell'elenco scrive «Già scaricato».
Accettazione:
- [unit] Dopo un download riuscito il brano è nell'archivio; un secondo download dello stesso link con lo stesso formato non avvia yt-dlp e finisce con «Già scaricato».
- [unit] Se il file è stato cancellato, o il formato è diverso (video invece di audio, MP3 invece di M4A), il brano si riscarica.
- [unit] Con «Salta i brani già scaricati» spento si riscarica sempre.
- [manual] Sul Mac: scarica un album, poi riscaricalo; ogni brano compare subito come «Già scaricato».
UX:
- Posto: nella riga del download (cambia lo stato di una riga); l'opzione in «Opzioni avanzate» (di rado)
- Stati: «Già scaricato · 03 - Titolo.m4a» in verde
- Termini nuovi: Già scaricato (Already downloaded)
Stato: fatta

### S-003 Salvare la musica in cartelle Artista / Album
Come utente, quando scarico audio i brani vanno in `Artista/Album/01 - Titolo.m4a`,
come in iTunes e Plex; posso spegnerlo in «Opzioni avanzate».
Accettazione:
- [unit] Un album di YouTube Music (playlist OLAK5uy_) va in «Artista/Album/01 - Titolo», con l'artista senza « - Topic».
- [unit] Un brano singolo usa i metadati di yt-dlp: «artista/album/titolo», con «Singoli» se manca l'album.
- [unit] Una playlist che non è un album resta nella cartella con il nome della playlist; i video e l'opzione spenta restano come prima.
- [snap] «Ascolta» mostra gli album dentro le cartelle degli artisti, con l'artista preso dalla cartella se i file non hanno il tag.
- [manual] Sul Mac: scarica un album e un brano da YouTube Music e controlla le cartelle.
UX:
- Posto: casella in «Opzioni avanzate», attiva di serie
- Termini nuovi: Cartelle Artista / Album (Artist / Album folders), Singoli (Singles)
Stato: fatta

## Milestone 2: libreria ordinata in «Ascolta» (0.4.0)

S-004, S-005

### S-004 Vedere la musica raggruppata per artista
Come utente, in «Ascolta» vedo un gruppo richiudibile per ogni artista, in ordine alfabetico,
con i suoi album dal più recente e i singoli in fondo, segnati «Singolo».
Accettazione:
- [unit] L'artista del gruppo è la cartella dell'artista (`Artista/Album`), altrimenti il primo artista dei tag («Renga Nek, Francesco Renga, Nek» → «Renga Nek»).
- [unit] Ordine: artisti alfabetici, poi album (più di 3 brani) dal più recente, poi singoli; senza artista e brani sciolti in fondo.
- [snap] Ogni album mostra anno, «Singolo» se lo è, l'artista solo se diverso dal gruppo, brani e durata.
- [manual] Sul Mac: la libreria vera appare per artista e i due «Cani Sciolti» si distinguono.
UX:
- Posto: nel riquadro di «Ascolta», gruppi richiudibili (`CollapsingHeader`)
- Termini nuovi: Uscita (Release), Singolo (Single)
Stato: fatta

### S-005 Cercare nella libreria
Come utente, scrivo in «Cerca nella libreria» e vedo solo gli artisti, album o brani che contengono il testo.
Accettazione:
- [snap] «cani» mostra solo i due «Cani Sciolti», nei loro gruppi aperti.
- [snap] Nessun risultato: «Nessun artista, album o brano con «…».»
UX:
- Posto: campo in cima all'elenco degli album
Stato: fatta

## Milestone 3: download più comodi (0.5.0)

S-006, S-007, S-008, S-009

### S-006 Agire su un singolo brano dell'elenco
Come utente, su ogni riga dei download ho l'azione giusta: «Annulla» mentre scarica,
«Mostra nel Finder» quando è finito, «Riprova» se non è riuscito.
Accettazione:
- [unit] «Annulla» su un brano ferma solo quello; «Riprova» lo riscarica con la stessa cartella e lo stesso nome.
- [snap] Righe in corso con «Annulla», finite con «Mostra nel Finder» (su Linux «Apri la cartella»), annullate con «Riprova»; in basso «Riprova i non riusciti (N)».
- [manual] Sul Mac «Mostra nel Finder» apre il Finder con il file selezionato.
Stato: fatta

### S-007 Sapere quando il download è finito
Come utente, se Grabbit è in secondo piano, a fine download ricevo una notifica e l'icona
nel Dock saltella; durante il download il titolo della finestra mostra la percentuale.
Accettazione:
- [manual] Sul Mac, con un'altra app davanti: notifica «Download finito: N scaricati» e icona che saltella.
- [manual] Il titolo della finestra è «Grabbit — 45%» durante il download, «Grabbit» alla fine.
Stato: fatta

### S-008 Aggiungere il link appena copiato
Come utente, quando torno su Grabbit dopo aver copiato un link YouTube o Spotify, Grabbit
me lo propone con «Aggiungi» e «No, grazie».
Accettazione:
- [unit] Si propone solo un link YouTube, YouTube Music o Spotify, da solo; non un testo qualsiasi.
- [snap] L'avviso compare nel riquadro «Link»; «Aggiungi» mette il link nel campo, «No, grazie» lo chiude.
Stato: fatta

### S-009 Vedere le copertine senza deformazioni
Come utente, in «Ascolta» le copertine sono quadrate, senza bande e non schiacciate.
Accettazione:
- [unit] Una copertina 16:9 di YouTube diventa il quadrato centrale.
- [unit] Per l'audio yt-dlp salva nei file la copertina già quadrata (filtro crop di ffmpeg).
- [manual] Sul Mac «25» e «19» di Adele hanno la copertina giusta.
Stato: fatta

## Milestone 4: libreria e player più comodi (0.6.0)

S-010, S-011

### S-010 Vedere nei risultati cosa ho già
Come utente, nei risultati della ricerca gli album e i brani che ho già nella libreria sono
segnati «Già nella libreria», con «Ascolta» che li apre nella scheda Ascolta.
Accettazione:
- [unit] Un risultato si ritrova nella libreria per titolo e artista, senza badare a maiuscole, punteggiatura o un anno davanti alla cartella («2002.Tracce»).
- [snap] Il brano che c'è mostra «Già nella libreria · Ascolta», quello che manca no.
- [manual] Sul Mac, nella discografia di un artista gli album già scaricati sono segnati.
Stato: fatta

### S-011 Ascoltare in modo più comodo
Come utente, nel player ho «Casuale» e «Ripeti» (tutto o il brano), la barra spaziatrice e
le frecce, «Ascolta tutto» per un artista, e alla riapertura ritrovo il brano dove l'avevo lasciato.
Accettazione:
- [unit] Il mescolamento tiene tutti i brani una volta sola.
- [snap] Barra del player con «Casuale» e «Ripeti brano» attivi, leggibili; «Ascolta tutto (N brani)» sugli artisti con più uscite.
- [snap] Dopo un riavvio la barra mostra l'ultimo brano in pausa alla posizione salvata, con le stesse scelte di Casuale e Ripeti.
- [manual] Sul Mac: spazio mette in pausa, le frecce cambiano brano, «Ripeti tutto» ricomincia l'album.
Stato: fatta

## Milestone 5: libreria e ascolto curati (0.7.0)

S-012, S-013, S-014

### S-012 Gestire un album da «Ascolta»
Come utente, nella pagina di un album ho «Mostra nel Finder», «Aggiungi a Musica» (iTunes sui
Mac meno recenti) e «Sposta nel Cestino…», che chiede conferma.
Accettazione:
- [snap] I pulsanti sotto «Ascolta l'album»; niente Cestino per i «Brani sciolti».
- [snap] La conferma nomina l'album; dopo, la cartella è nel Cestino e l'elenco si aggiorna con «“…” è nel Cestino.».
- [manual] Sul Mac: «Aggiungi a iTunes» importa l'album; dal Cestino «Ripristina» lo rimette a posto.
Stato: fatta

### S-013 Scegliere i brani di un album prima di scaricarlo
Come utente, negli album dei risultati premo «Scegli i brani…», tolgo la spunta a quelli che
non voglio e scarico solo gli altri, con il loro numero nell'album.
Accettazione:
- [unit] Solo i brani scelti si scaricano, come «01 - …» e «03 - …».
- [snap] L'elenco con le caselle, «Tutti», «Nessuno» e «Scarica i brani scelti (N)».
- [manual] Sul Mac, con un album vero di YouTube Music.
Stato: fatta

### S-014 Ascoltare tutti i brani allo stesso volume
Come utente, con «Volume uguale tra i brani» (attivo di serie) i brani di album diversi
suonano allo stesso volume, senza che i file vengano toccati.
Accettazione:
- [unit] Un brano forte si abbassa, uno piano si alza (al massimo 6 dB e mai oltre il picco); i silenzi non contano.
- [unit] Un file vero si misura e la misura resta nell'archivio.
- [snap] La casella in «Ascolta»; i brani della libreria si misurano in sottofondo.
- [manual] Sul Mac, passando da un album vecchio a uno recente il volume resta simile.
Stato: fatta

### S-015 Guardare un video musicale dalla libreria
Come utente, un video musicale scaricato compare in «Ascolta» segnato «Video» e, cliccandolo,
si apre nel player video del computer (QuickTime sul Mac).
Accettazione:
- [unit] Un file video (MP4 con una traccia video, MKV, MOV, M4V) è segnato come video; la durata viene dalla traccia audio.
- [snap] La riga mostra «Video» e la durata; «Ascolta l'album», Casuale e «Ascolta tutto» saltano i video.
- [manual] Sul Mac il video si apre in QuickTime e la musica di Grabbit si mette in pausa.
Stato: fatta

## Milestone 6: video da ogni sito, con sottotitoli (0.8.0)

Dal piano `docs/PIANO-REELDOWNLOADER.md`. S-016, S-017, S-018

### S-016 Sottotitoli e trascrizione dei video
Come utente, in modo Video spunto «Sottotitoli e trascrizione», scelgo le lingue (italiano e
inglese di serie) e ottengo i sottotitoli dentro il video, in .srt e una trascrizione .txt pulita.
Accettazione:
- [unit] Con la casella attiva yt-dlp riceve `--write-subs --write-auto-subs --sub-langs it.*,en.* --convert-subs srt --embed-subs`; spenta o in audio, niente.
- [unit] Un .srt con numeri, orari, etichette e righe ripetute diventa un testo pulito; solo i sottotitoli del video appena scaricato.
- [snap] La casella e il menu delle lingue in modo Video.
- [manual] Sul Mac un video RaiPlay con sottotitoli e uno YouTube con sottotitoli automatici: «Apri la trascrizione» apre il .txt.
Stato: fatta

### S-017 Scegliere il nome del file
Come utente, con un solo link compare «Nome del file (facoltativo)» e il video si salva con quel nome.
Accettazione:
- [unit] Il nome, pulito dai caratteri non ammessi, diventa il nome del file nelle stesse cartelle.
- [snap] Il campo compare con un solo link, non con due.
Stato: fatta

### S-018 Video nella cartella Filmati, per sito
Come utente, i video vanno nella cartella Filmati (Grabbit), in una cartella per sito e per programma
o canale: Facebook/…, RaiPlay/<programma>/…, YouTube/<canale>/…; la musica resta dov'era.
Accettazione:
- [unit] Il sito si riconosce dal link (Facebook, fb.watch, RaiPlay, RaiPlay Sound, YouTube, Vimeo, un sito qualsiasi).
- [unit] Video in `<cartella video>/<sito>/<programma o canale>/Titolo`; audio nella cartella della musica; opzione spenta come prima.
- [snap] «Salva in» mostra la cartella del modo scelto; «Video in cartelle per sito» nelle opzioni avanzate.
Stato: fatta

## Milestone 7: accesso ai siti e messaggi chiari (0.9.0)

Dal piano `docs/PIANO-REELDOWNLOADER.md`. S-019, S-020, S-021

### S-019 Accesso ai siti
Come utente, in «Opzioni avanzate › Accesso ai siti» scelgo il browser e premo «Aggiorna l'accesso»
per Facebook, Instagram o RaiPlay: Grabbit copia i cookie di quel solo sito e li usa solo per i suoi link.
Accettazione:
- [unit] Si tengono solo i cookie dei domini del sito (anche le righe «#HttpOnly_»); file privato (600); nessun file temporaneo rimasto.
- [unit] Un link del sito riceve `--cookies <file del sito>`, gli altri il browser di «Usa i cookie del browser» o niente.
- [snap] Sezione con il browser, una riga per sito («non impostato», «aggiornato oggi»), «Aggiorna l'accesso», «Togli», esito in verde o rosso.
- [manual] Sul Mac, un reel di Facebook che senza accesso non si scarica.
Stato: fatta

### S-020 Sito nell'elenco e messaggi chiari
Come utente, ogni riga dei download dice il sito, e gli errori comuni hanno un messaggio con il rimedio.
Accettazione:
- [unit] DRM, accesso richiesto (con il rimedio giusto per il sito), blocco per paese, robot di YouTube, link non supportato, contenuto rimosso.
- [snap] «RaiPlay», «RaiPlay Sound» davanti al titolo nella riga.
Stato: fatta

### S-021 RaiPlay Sound in audio, nella cartella Podcast
Come utente, i link RaiPlay Sound si scaricano sempre come audio in `Musica/Grabbit/Podcast/<programma>/`,
e in «Ascolta» stanno nel gruppo «Podcast».
Accettazione:
- [unit] Un link RaiPlay Sound è audio, nella cartella della musica, in «Podcast/<programma>/Titolo».
- [manual] Sul Mac, una puntata di un programma RaiPlay Sound.
Stato: fatta

## Milestone 8: scheda File e limite di velocità (0.10.0)

Dal piano `docs/PIANO-DROPALL.md`, sezione 7 (file normali: sì; torrent: dopo una prova su High Sierra;
icona nella barra dei menu e nel Dock: in una versione successiva). S-022, S-023

### S-022 Scheda «File»: download manager
Come utente, nella scheda «File» incollo uno o più indirizzi http(s) di file e Grabbit li scarica con più
connessioni, in code che posso avviare, fermare, rinominare e pianificare, con pausa e ripresa.
Accettazione:
- [unit] I 16 test del download manager di Dropall: nome del file, ripresa dal punto raggiunto, segmenti, code, pianificazione, limite condiviso.
- [unit] Un server che accetta 2 connessioni e risponde 429 alle altre (come ftp.crifo.org): il file arriva intero, senza «Riprova».
- [snap] Scheda con indirizzi, «Salva in», coda, connessioni; riga con barra, velocità, tempo rimasto, «Pausa», «Apri cartella».
- [snap] Stato di errore con «Riprova».
- [manual] Sul Mac, un file grande: pausa, chiusura di Grabbit, riapertura e ripresa.
Stato: fatta

### S-023 Limite di velocità per tutti i download
Come utente, imposto un limite in KB/s che vale per i file e anche per video e musica, così gli altri
in casa possono usare la rete.
Accettazione:
- [unit] yt-dlp riceve `--limit-rate`, diviso tra i download che vanno insieme; niente limite se spento.
- [snap] Con 1024 KB/s un file si scarica a circa 1020 KB/s.
- [manual] Sul Mac, un video con il limite acceso.
Stato: fatta

## Milestone 9: torrent tramite Transmission (0.11.0)

Grabbit non contiene un motore torrent (libtorrent in C++ non parte su High Sierra): comanda
Transmission già installato con la sua interfaccia remota. S-024

### S-024 Torrent e magnet nella scheda «File»
Come utente, incollo un link magnet o un indirizzo .torrent (o scelgo un file .torrent) nella scheda
«File»: Grabbit lo passa a Transmission e mostra avanzamento, velocità e peer, con pausa e ripresa.
Accettazione:
- [unit] Magnet, indirizzi e file .torrent riconosciuti; nome dal `dn=` del magnet.
- [unit] Con un finto Transmission: sessione (409), aggiunta con la cartella di «Salva in», file .torrent in base64, elenco, pausa, ripresa, «Togli» senza cancellare i file, limite di velocità.
- [unit] Transmission spento: messaggio con il rimedio; il link si apre con l'app predefinita per i torrent.
- [snap] Riquadro «Torrent» con righe (barra, velocità, peer) e stato «Transmission non risponde».
- [manual] Sul Mac, un magnet di una distribuzione Linux.
Stato: fatta

## Milestone 10: torrent anche con qBittorrent (0.12.0)

S-025

### S-025 qBittorrent nel riquadro «Torrent»
Come utente che usa anche qBittorrent, scelgo «Aggiungi a: Transmission / qBittorrent» e vedo nello
stesso riquadro i torrent di tutte e due le app, ognuno con il nome della sua app.
Accettazione:
- [unit] Con un finto qBittorrent: magnet e cartella in multipart, file .torrent caricato, elenco (stati, tempo infinito, peer), «stop»/«start» con ripiego su «pause»/«resume» (qBittorrent 4), «Togli» senza cancellare i file, limite in byte/s.
- [unit] Password richiesta (403): messaggio con «Salta autenticazione per i client in localhost».
- [snap] Righe delle due app insieme con l'etichetta dell'app; l'app non scelta e chiusa non mostra avvisi.
- [manual] Sul Mac, un magnet aggiunto a qBittorrent.
Stato: fatta

## Milestone 11: barra dei menu e Dock (0.13.0, poi 0.14.0)

Dal mockup «Grabbit: barra dei menu e Dock». S-026 (passo 1), S-027 (passo 2)

### S-026 Icona nella barra dei menu e numero sul Dock
Come utente del Mac, vedo nella barra dei menu l'avanzamento di tutti i download (video e musica,
file, torrent) e dal suo menu scarico il link copiato, metto in pausa file e torrent, scelgo il
limite di velocità, apro Grabbit o la cartella dei download; sul Dock vedo quanti download sono in corso.
Accettazione:
- [unit] Icone ferma, in download (barretta a passi), finito, non riuscito; percentuale e prima riga del menu.
- [unit] Link copiato: magnet e .torrent all'app torrent, .iso/.zip/.dmg… alla scheda «File», il resto a yt-dlp.
- [unit] Ogni voce del menu ha un id che torna alla sua azione.
- [ci] Compila per macOS (Rust puro sulle API di AppKit, nessun codice C).
- [manual] Sul Mac (High Sierra): icona, menu, limite, «Scarica il link copiato», numero sul Dock.
Stato: fatta

### S-027 Barra sull'icona del Dock e menu del tasto destro
Come utente del Mac, l'icona di Grabbit nel Dock ha una barra di avanzamento mentre scarica, e con il
tasto destro trovo le stesse voci del menu della barra dei menu.
Accettazione:
- [unit] Icona del Dock 256×256 con la barra (bianca fino all'avanzamento, fondo scuro dopo), il logo sopra resta uguale.
- [ci] Compila per macOS: `setApplicationIconImage` e `applicationDockMenu:` aggiunto al delegato di winit.
- [manual] Sul Mac (High Sierra): barra sull'icona durante un download, icona normale alla fine, menu del tasto destro che funziona.
Stato: fatta

### S-028 Magnet e file diretti incollati in «Scarica»
Come utente, se incollo un magnet, un .torrent o un file diretto (.iso, .zip…) nella scheda «Scarica»,
Grabbit non lo passa a yt-dlp (che darebbe errore) ma all'app torrent o alla scheda «File», e apre «File».
Accettazione:
- [unit] I link si dividono in video e musica, torrent, file diretti; le ricerche restano a yt-dlp.
- [snap] Magnet incollato in «Scarica»: si apre «File» con «Aggiunto a Transmission: <nome>».
Stato: fatta

## Milestone 12: ascolto più comodo, licenze, Grabbit in secondo piano (0.14.0)

S-029, S-030, S-031, S-032

### S-029 Artisti chiusi in «Ascolta»
Come utente con una libreria grande, in «Ascolta» vedo l'elenco degli artisti chiusi e apro con la
freccia solo quello che mi interessa; mentre cerco, i gruppi trovati restano aperti.
Accettazione:
- [snap] Dopo la lettura della cartella si vedono solo «Artista · N uscite»; la freccia apre gli album.
Stato: fatta

### S-030 Mini player
Come utente che ascolta musica, con «Mini player» la finestra si riduce alla sola barra del player;
con «Mostra tutto» torna com'era.
Accettazione:
- [snap] La finestra diventa alta quanto la barra, con «Mostra tutto»; poi torna alla grandezza di prima.
- [snap] Se non c'è più niente in ascolto, torna da sola la finestra intera.
Stato: fatta

### S-031 Licenze e crediti
Come utente, da «Licenze e crediti» in fondo alla finestra vedo la licenza di Grabbit, i crediti
(Dropall), i programmi che Grabbit scarica (yt-dlp, FFmpeg, Deno, QuickJS) e le licenze di tutte le
librerie incluse.
Accettazione:
- [unit] Ogni libreria di Cargo.lock è in `assets/licenses.txt` (generato da `scripts/licenses.py`).
- [snap] Finestra con le sezioni, i link e l'elenco per licenza; «Chiudi» o Esc la chiudono.
Stato: fatta

### S-032 Grabbit in secondo piano a finestra chiusa (Mac)
Come utente del Mac, chiudendo la finestra Grabbit resta nella barra dei menu e i download continuano;
la riapro da «Apri Grabbit» o cliccando l'icona nel Dock, ed esco con «Esci da Grabbit» o Cmd+Q.
Accettazione:
- [ci] Compila per macOS (`applicationShouldHandleReopen:hasVisibleWindows:` aggiunto al delegato di winit).
- [manual] Sul Mac: chiusura con il pallino rosso (notifica la prima volta), download che continua, riapertura dal menu e dal Dock, «Esci da Grabbit».
- [manual] Opzioni avanzate › «Chiudendo la finestra resta nella barra dei menu» spenta: Grabbit si chiude come prima.
Stato: fatta

### S-033 Scansione incrementale della musica
Come utente con molta musica, «Ascolta» non rilegge ogni volta tutti i brani: come un backup
incrementale, Grabbit ricorda i dati di ogni file (con dimensione e data di modifica) e riapre solo i
file nuovi o cambiati; le copertine già rimpicciolite restano in cache.
Accettazione:
- [unit] Seconda scansione senza file riaperti, stesso elenco e stessa copertina; un file cambiato si rilegge, uno cancellato sparisce.
- [manual] Sul Mac, con la libreria vera: la seconda apertura di «Ascolta» è molto più veloce.
Stato: fatta

### S-034 Avanzamento della lettura della cartella
Come utente, mentre «Ascolta» legge la cartella della musica vedo a che punto è: «Leggo la cartella…
N di M brani» con la barra e la percentuale.
Accettazione:
- [unit] L'avanzamento parte da 0 e arriva al totale dei brani.
- [snap] Durante la lettura: testo «20 di 200 brani» e barra al 10%.
Stato: fatta

### S-035 Togliere i torrent completati
Come utente, nel riquadro «Torrent» premo «Togli i completati (N)» e i torrent finiti escono dall'app
torrent (smettono di essere condivisi); i file scaricati restano nella cartella.
Accettazione:
- [snap] Il pulsante compare solo se c'è almeno un torrent al 100%; dopo il clic restano quelli in corso, con «Tolti N torrent completati».
Stato: fatta
