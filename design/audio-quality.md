# Vérification audio locale

Dans la bibliothèque, cliquer sur **Vérifier** à côté d’un morceau local.
La lecture ne démarre pas et le fichier n’est pas modifié. Le travail est exécuté
en arrière-plan ; Retour à la bibliothèque ferme le rapport sans annuler le calcul.
Une seule analyse peut être active à la fois.

Le programme `flac` doit être installé pour vérifier un FLAC natif. Il est déjà
présent sur la machine de développement. Aucun nouveau paquet Rust ou Python
n’est requis. Le programme externe est appelé avec des arguments séparés, sans
shell. Il n’est pas redistribué par Onde ; s’il est empaqueté avec Onde, respecter
sa licence GPL et ses notices (libFLAC a une licence distincte BSD).

## Mesures et interprétation

- Signature `fLaC`, puis test complet `flac --test` : erreurs
  du flux, CRC et vérification MD5 lorsqu’une empreinte est présente. Un échec est présenté avec le diagnostic du décodeur. Une empreinte MD5
  absente est indiquée comme non vérifiable, sans empêcher le spectre ; les autres
  avertissements sont affichés sans être assimilés à une corruption. Délai maximal 120 secondes.
- Spectre moyen et spectrogramme du premier canal, jusqu’à 300 secondes à partir
  du début. FFT de 4096 points avec fenêtre Hann, 128 bandes linéaires. Le spectre
  moyen agrège la puissance de toutes les fenêtres ; le spectrogramme conserve
  une fenêtre par seconde et n’est donc pas une analyse exhaustive temporelle.
- Axe fréquentiel de 0 à Nyquist ; couleur et hauteur représentent une puissance
  relative au plein niveau entre −120 et 0 dB. Ce n’est pas une mesure acoustique
  calibrée. Les autres canaux ne sont pas analysés spectralement.
- Une coupure haute fréquence peut provenir d’une compression avec perte, mais
  aussi du microphone, du mastering, du contenu ou d’un rééchantillonnage.
  Même un FLAC intègre peut contenir de l’audio précédemment compressé en MP3.
  Le rapport ne délivre pas de certification d’origine sans perte.

Références : [outil FLAC officiel](https://www.xiph.org/flac/documentation_tools_flac.html),
[format FLAC](https://www.rfc-editor.org/rfc/rfc9639.html).

## Réception Telegram

Les nouveaux fichiers utilisent `Artiste - Album - Titre.ext` lorsque les tags
Vorbis FLAC sont présents, puis les attributs audio Telegram ou le nom fourni en
repli. Séparateurs de chemin et caractères dangereux sont neutralisés ; collisions
résolues par suffixe numéroté, sans écrasement. Les tags ne sont pas modifiés.

Les blocs de téléchargement sont portés à 512 Kio et le débit moyen est affiché.
Le backend AES observé utilise déjà OpenSSL. Aucun gain réseau chiffré n’a été
mesuré : le débit dépend aussi de Telegram, du réseau et des limites du compte.

## Validation

Deux tests Rust ciblés couvrent le spectre d’un signal WAV, le rejet d’une fausse
identification FLAC, l’intégrité d’un FLAC encodé avec l’outil officiel et le rejet
d’un fichier corrompu. Neuf tests Python couvrent le pont Telegram, dont les noms
issus des tags et la neutralisation des chemins. Le rendu graphique du rapport
reste à valider visuellement dans l’application.
