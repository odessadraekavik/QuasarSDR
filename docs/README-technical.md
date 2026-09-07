# QuasarSDR — réception USB uniquement

Application Rust Windows : sélection d'un SDR USB réel, FFT/waterfall, jusqu'à
8 VFOs, démodulation et sélection audio prioritaire. Aucun mode simulation,
aucun serveur rtl_tcp, aucune fonction d'émission RF.

## Démarrage

```powershell
cd C:\Users\Odessa\Desktop\QuasarSDR
cargo run --release --locked
```

Ou ouvrir `target\release\quasar-sdr.exe`. Conserver son sous-dossier `drivers`.
Le script `build-windows.ps1` compile et copie les bibliothèques locales au bon endroit.
Rust stable x64 MSVC et Visual Studio Build Tools C++/SDK Windows sont requis pour compiler.

## Sélection USB

L'interface Quasar utilise des boutons toggle : cyan = actif, gris = inactif.
Le bouton **RX ACTIF / DÉMARRER RX** permet d'arrêter et de reprendre la réception.
Les commandes essentielles occupent le bandeau supérieur ; **Réglages du canal**
déplie les gains audio, correction SQL, priorité et CTCSS de chaque carte VFO.
Cliquer une carte sélectionne le canal ; sa fréquence se saisit directement dans
le grand afficheur ambre, puis se valide avec **ACCORDER** ou Entrée.

Le **meter du canal sélectionné** affiche la puissance intégrée en dBFS et le
contraste au-dessus du bruit local. Sa barre est animée entre les snapshots réels
(rafraîchissement UI visé ~60 Hz) : attaque 35 ms, relâchement 350 ms, crête tenue
une seconde puis décroissance de 12 dB/s. Sans mesure, aucune valeur n'est inventée.
Ce meter n'est pas un S-mètre ou une mesure dBm calibrée.

Raccourcis visuels : **RELATIF** = normalisation, **DC** = masquage du pic central,
**FOND** = masquage du bruit, **AUTO ZÉRO** = calibration 2 s, **VOIX IA** = RNNoise.
Les fonctions détaillées ci-dessous restent accessibles via ces toggles.

- La liste **SDR USB** contient les appareils physiques reconnus par le backend RTL-SDR.
- Choisir un appareil démarre sa réception. **Reconnecter** réessaie en cas d'erreur.
- Aucun appareil : dropdown vide et désactivé, message rouge
  **AUCUN SDR COMPATIBLE CONNECTÉ**, réglages grisés, aucun flux audio ou DSP lancé.
- Avec un appareil détecté mais pas encore sélectionné, les réglages restent grisés.
- L'énumération s'effectue hors du thread UI, toutes les deux secondes.
- Le débranchement ou une erreur USB arrête l'audio et efface le spectre/waterfall.
- Les réglages s'activent à l'arrivée des premières données réelles.
- Au démarrage : un VFO **VHF marine canal 16, 156,800 MHz**, en NFM
  (bande 25 kHz, déviation de référence 5 kHz).
  Référence : [table officielle des canaux VHF](https://www.navcen.uscg.gov/us-vhf-channel-information).
- **SQL relatif** activé par défaut : **+10 dB au-dessus du bruit local**, commun
  à tous les VFOs. Chaque canal dispose d'une correction SQL, initialement 0 dB.
  Décocher SQL relatif sélectionne le seuil absolu (−50 dBFS initialement), mesuré
  sur la puissance intégrée du canal. Les deux modes sont alternatifs.
- **GAIN RF** : **+28 dB** par défaut pour le Blog V4, commande du tuner en mode
  manuel. Le menu propose uniquement les pas de gain retournés par le matériel.
  Le gain effectivement appliqué est affiché à côté ; il peut être ajusté en RX.
- Si le SDR est occupé par SDR++, SDR# ou un autre logiciel, l'erreur est affichée.

Le backend actuellement implémenté couvre la famille **RTL-SDR**, dont le **Blog V4**.
Les autres familles (Airspy, SDRplay, etc.) nécessitent leurs propres backends.
Chaque entrée affiche son nom, son index USB et son numéro de série si disponible.
Windows peut empêcher une seconde lecture des descripteurs pendant la réception :
le nom du handle actif est alors conservé, sans ajouter de périphérique fictif.
La disponibilité du flux reste contrôlée indépendamment. Une identité modifiée
après réénumération nécessite une nouvelle sélection.

## Bibliothèques et pilote Windows

Les bibliothèques x64 déjà présentes dans les téléchargements de cette machine
ont été copiées dans `drivers/` et `target/release/drivers/` :
`rtlsdr.dll`, `msvcr100.dll`, `pthreadVC2.dll`.
Aucun driver système n'a été installé ou modifié.

Sur une autre machine, fournir une bibliothèque RTL-SDR **compatible V4** et
associer la clé au pilote **WinUSB** si ce n'est pas déjà fait.
Le guide officiel explique la procédure : [RTL-SDR Blog V4](https://www.rtl-sdr.com/v4/).
Les DLL sont locales et ignorées par Git ; une CI sans ces fichiers compile mais
son exécutable nécessite le runtime décrit dans `drivers/README.md`.
Un runtime manquant est signalé séparément de la liste vide.

SDROxide adopte une autre implémentation : driver RTL-SDR écrit en Rust et
bibliothèque USB `nusb` embarqués. Cela évite `rtlsdr.dll`, mais leur documentation
exige également WinUSB sous Windows. QuasarSDR utilise pour le moment librtlsdr
avec sa propre interface Rust RX ; aucun code de SDROxide n'a été copié.
[Source SDROxide](https://github.com/dividebysandwich/sdroxide/blob/main/Cargo.toml)
— [préparation Windows](https://github.com/dividebysandwich/sdroxide#rtl-sdr-permissions).

## Chaîne de réception

```text
RTL-SDR USB (2,400 MS/s)
  -> librtlsdr read_async : buffers I/Q U8, file bornée
  -> worker acquisition : conversion complexe, toute la bande I/Q conservée
  -> 2,4 MS/s, blocs 25 600 -> file crossbeam bornée (4 blocs)
  -> worker DSP
       +-> Hann -> FFT -> quantile local -> plancher de bruit -> squelch
       +-> I/Q -> NCO par VFO -> FIR 2,4M/240k/48k -> démodulation -> CTCSS/VAD
                                     -> priorité / solo / mute / mix
                                     -> rampe de gain -> mono 48 kHz
                                     -> file bornée -> CPAL -> haut-parleurs
  -> snapshots FFT/états à ~33 Hz -> egui GPU
```

La FFT mesure les signaux ; la démodulation exploite les I/Q temporels, avec des
états conservés entre blocs. Le callback audio n'alloue pas et ne bloque pas.
Les files sont bornées. Un trou I/Q DSP réinitialise les filtres ; une perte en
amont dans la file USB arrête la réception avec un diagnostic. Le temps réel
reste souple, sans garantie de latence Windows. La fermeture annule les transferts
USB asynchrones avant de fermer le handle ; aucun `read_sync` indéfiniment bloquant.

## Fréquences, déplacement de bande et DC

Chaque VFO possède une fréquence absolue éditable en **MHz**. Saisir `156.800`
(ou `156,800`), puis **Entrée** ou **Accorder**. La fréquence du VFO sélectionné
est affichée en grand en haut. Le champ **Centre RX** permet aussi un accord direct.
La plage acceptée pour le Blog V4 est 0,500 à 1766 MHz ; les autres tuners peuvent
refuser certaines fréquences, auquel cas le driver remonte une erreur.

L'acquisition et l'affichage couvrent **2,40 MHz**, à **2,40 MS/s** : il n'y a plus
la réduction globale à 192 kHz. Chaque VFO possède sa propre chaîne multidébit.
Une marge de 2 % est réservée aux bords pour les VFOs, où la réponse du tuner se dégrade.
La FFT de 2048 points donne des bins de 1171,875 Hz sur cette vue large ; la mesure
porte sur les 2048 premiers échantillons de chaque bloc. Un zoom FFT plus fin reste
une évolution possible.

- **Clic** sur le spectre ou le waterfall : accorder le VFO sélectionné.
- **Glisser horizontalement** sur l'une des surfaces : déplacer toute la bande.
  Le nouveau centre est prévisualisé ; le matériel est réaccordé au relâchement.
- Une fréquence VFO hors de la bande actuelle recentre automatiquement le récepteur.
- Les autres VFOs gardent leur fréquence absolue. Ceux hors de la capture deviennent
  **HORS BANDE**, sans audio ; le bouton **Centrer sur ce VFO** les remet dans la vue.
- Un seul tuner ne peut pas écouter simultanément des VFOs séparés de plus que sa
  bande instantanée. Le pan ne constitue pas un balayage automatique entre bandes.
- **Masquer le pic DC**, activé par défaut : interpolation visuelle de cinq bins
  centraux (~5,86 kHz) du spectre et du waterfall. Cela ne retire pas le DC des I/Q,
  ne modifie ni le squelch ni la démodulation et peut masquer un vrai signal au centre.

Au réaccord, les transferts USB sont arrêtés puis redémarrés avec un nouveau centre
confirmé par le driver. Les blocs portent leur configuration et une génération ;
les blocs périmés sont écartés. Le waterfall et le spectre sont vidés pour ne pas
présenter un ancien signal sur un nouvel axe de fréquences. Il peut y avoir une
courte interruption audio pendant cette opération.

Le gain RF manuel pilote le tuner ; les gains VFO restent des gains audio.
Aucune API TX, écriture EEPROM ou commande bias tee n'est exposée.

## Modules

| Module | Rôle |
|---|---|
| `driver/usb.rs` | Énumération physique, ABI RX, transferts USB pleine bande et arrêt |
| `engine.rs` | Acquisition, DSP, files, erreurs et arrêt |
| `dsp/spectrum.rs` | FFT Hann, plancher local au quantile 25 %, lissage |
| `dsp/vfo.rs`, `filter.rs` | NCO, FIR, NFM/WFM/AM/LSB/USB |
| `dsp/detector.rs` | CTCSS Goertzel, VAD énergie |
| `dsp/mod.rs` | Squelch et routage audio prioritaire/mix |
| `audio.rs` | CPAL, adaptation de débit et sortie mono |
| `ui.rs` | Dropdown USB, état déconnecté, dashboard |
| `driver/test_signal.rs` | Échantillons de test uniquement, exclus du binaire normal |

## Fonctionnalités et limites

Priorité : plus petite valeur gagnante, maintien du VFO courant en cas d'égalité.
Mute exclut un canal ; Solo restreint les candidats. Mix normalise les canaux actifs.
Le squelch utilise le contraste au-dessus du plancher local, hystérésis 3 dB et
maintien ~140 ms. Ce contraste n'est pas un SNR RF intégré calibré.

La **Vue relative au bruit**, activée par défaut, normalise le spectre et le
waterfall avec la même soustraction : puissance par bin moins bruit local.
Le bruit se situe ainsi près de 0 dB, et un signal 10 dB au-dessus garde le même
niveau relatif quelle que soit la puissance absolue du bruit. Ces valeurs sont
des dB relatifs, pas des dBFS. La ligne SQL représente le seuil commun avant
correction individuelle. Le choix de vue ne change pas le mode de détection.
Il s'agit d'une implémentation du principe Dynamic Noise Floor, pas d'une copie
de l'algorithme Mykola : moyenne exponentielle des puissances FFT, quantile local
25 % compensé approximativement, puis suivi temporel asymétrique du bruit.
La convergence initiale prend quelques dizaines de trames. Cette normalisation
ne constitue pas une égalisation des volumes audio.

### Auto zéro et masquage du fond

**Auto zéro · 2 s** relance une acquisition rapide du profil de bruit local sur
deux secondes d'I/Q, puis conserve un suivi lent. La progression est affichée.
La calibration démarre aussi à la connexion, au réaccord et après changement de
gain RF. Les pics dépassant l'estimation locale de 6 dB sont exclus du suivi après
l'amorçage ; une bande largement occupée peut néanmoins fausser le profil.
Lancer de préférence la mesure pendant une pause des communications.

Le bouton active la vue relative et **Masquer le fond** : les niveaux inférieurs
à la marge choisie (+3 dB initialement) sont affichés à zéro dans les deux vues.
Cette option est visuelle et peut cacher des signaux faibles ; SQL et audio
continuent à utiliser les mesures originales. Elle ne supprime pas le bruit RF
dans les I/Q et n'est pas un noise blanker d'impulsions.

### Voix IA locale

**Voix IA · RNNoise local** active le modèle embarqué de
[nnnoiseless](https://github.com/jneem/nnnoiseless), port Rust de RNNoise.
Pas de service distant, de GPU requis ou de téléchargement de modèle au lancement.
L'option est désactivée au démarrage ; intensité initiale 80 %, réglable de 0 à 100 %.
Elle convient à la parole et peut altérer une voix faible, la musique ou les données.
Ce n'est pas un modèle spécialement entraîné sur les communications radio et la
qualité sur des voix SDR réelles reste à évaluer à l'écoute.

Chaque VFO ouvert utilise son propre état neuronal, après démodulation et mesures
CTCSS/VAD/SQL, avant gains audio et mixage. L'état est effacé à la fermeture du
squelch ou au réaccord. Les blocs 48 kHz de 512 échantillons sont adaptés aux trames
RNNoise de 480 échantillons, sans perte de durée. Le mélange original/traité est
aligné en temps ; latence ajoutée d'environ 20 ms quand l'option est active.
Décocher l'option restaure le chemin audio direct.

Diagnostic matériel avec IA et squelch forcé ouvert :
`cargo run --release --example usb_probe -- --engine --voice`.

NFM/AM fonctionnent comme démodulateurs de départ ; WFM est mono avec désaccentuation
50 µs, sans RDS/stéréo. LSB/USB utilisent un FIR complexe de bande latérale.
CTCSS surveille une tonalité configurée sur 200 ms et reste expérimental.
Le VAD détecte l'énergie, pas spécifiquement la parole. Le plancher peut être
biaisé par des émissions larges et denses. Le rééchantillonnage audio linéaire
reste à améliorer pour les sorties à très faible débit.

À développer : DCS, enregistrements WAV et pré-roll, 
balayage entre bandes, canalisation multidébit optimisée, UI WASM/WebSocket/WebAudio.
Le noyau DSP reste compilable séparément avec `--lib --no-default-features`.
Aucune vitesse de balayage en MHz/s n'est revendiquée.

## Vérifier

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
cargo build --release --locked
# Diagnostic réel (premier SDR connecté), uniquement en réception :
cargo run --release --example usb_probe
cargo run --release --example usb_probe -- --receive
# Noyau DSP seul :
cargo test --lib --no-default-features --locked
```

Les tests unitaires ne nécessitent pas de clé USB et n'ouvrent pas de matériel.
Le diagnostic séparé a vérifié le Blog V4 de cette machine : 2 560 000 échantillons
I/Q reçus à 2,4 MS/s au centre 156,800 MHz, puis arrêt et libération du handle USB.


