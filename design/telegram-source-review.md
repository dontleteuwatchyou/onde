# Source Telegram facultative

## Installation et utilisation

Linux, Python 3.10+ et un compte Telegram déjà ouvert sur un appareil sont
nécessaires. L’intégration se trouve dans l’écran Téléchargements d’Onde.
Sans session, le pont Python ne démarre pas tant que les permissions ne sont pas
acceptées. Si une session existe déjà, il démarre et s’arrête avec l’application,
et Onde se reconnecte au lancement, sans envoyer de message. Les dépendances Python sont
facultatives et ne sont pas installées par Onde.

Depuis la racine du dépôt :

```sh
python3 -m venv ~/.local/share/onde/telegram-venv
~/.local/share/onde/telegram-venv/bin/pip install -r integrations/telegram/requirements.txt
export ONDE_TELEGRAM_API_ID='identifiant-de-votre-application'
export ONDE_TELEGRAM_API_HASH='hash-de-votre-application'
cargo run
```

Obtenir les identifiants de l’application sur
[my.telegram.org](https://my.telegram.org), conformément aux
[prérequis API](https://core.telegram.org/api/obtaining_api_id).
Sans ces variables, Onde lit `~/.config/onde/telegram/api.env` (lignes
`ONDE_TELEGRAM_API_ID=…` et `ONDE_TELEGRAM_API_HASH=…`). Onde utilise
`ONDE_TELEGRAM_PYTHON` s’il est défini, sinon `~/.local/share/onde/telegram-venv`, sinon `python3`.
Ces identifiants ne sont ni le code de connexion ni le mot de passe 2FA.
Ne pas les publier ni les mettre dans le dépôt. Ils sont hérités par le processus
Python via l’environnement ; ils ne sont pas dans ses arguments.

1. Configurer le dossier musical dans Onde.
2. Lire les permissions dans Téléchargements. Refuser laisse la source inactive.
3. Accepter la connexion puis scanner le QR avec Telegram > Paramètres > Appareils.
4. Saisir une recherche ou un lien dans le champ de recherche de l’onglet Telegram, puis valider avec Entrée.
   Une recherche passe par une requête inline : réponse immédiate, rien n’est posté dans le chat.
   Cliquer sur une musique la télécharge dans le dossier musical dès que le bot l’a préparée.
   Sélectionner l’onglet ne transmet rien ; les autres sources restent indépendantes. Envoyer `/start` reste une action
   explicite si le bot le demande.
5. Consulter progression, réponse textuelle, erreurs et fichiers reçus dans le
   panneau Telegram. La bibliothèque est rafraîchie après réception.
6. Tous les audios envoyés par le bot après la recherche sont récupérés
   automatiquement pendant cinq minutes.
7. Arrêter la réception annule le transfert et ferme la fenêtre de réception.
   Déconnecter ferme la connexion et conserve la session locale. Révoquer invalide
   la session auprès de Telegram et nécessite une connexion active. Supprimer la
   session demande une seconde confirmation et efface les fichiers locaux.

La suppression locale n’est pas une révocation distante : si nécessaire,
révoquer aussi la session dans Telegram > Appareils. Les fichiers musicaux déjà
reçus restent conservés. Fermer Onde arrête le pont et sa connexion.

## Permissions et confidentialité

L’écran explique avant connexion qu’Onde garde une session locale donnant accès
au compte et que le bot tiers reçoit les recherches, liens et informations de
profil exposées par Telegram. La restriction à ce bot est une règle d’Onde,
pas une permission Telegram limitant la session à cette conversation.

Session : dossier `onde/telegram` sous le répertoire de configuration système
(généralement `~/.config/onde/telegram`). Répertoire 0700 et fichiers de session
0600. Un verrou interdit deux ponts simultanés. Protéger également les sauvegardes
et le compte système : la session n’est pas chiffrée par Onde au repos.

Aucun champ ni journal de code de connexion ou de mot de passe 2FA. Les jetons QR
sont transitoires, uniquement affichés dans l’interface. Si Telegram exige la 2FA,
le parcours est interrompu sans demander le mot de passe. Ne pas désactiver la
2FA pour utiliser cette fonction. Les journaux Telethon sont désactivés ; les
exceptions sont remplacées par des messages contrôlés. Les réponses du bot et
chemins reçus restent en mémoire dans le panneau, sans historique persistant.

## Conditions et droits examinés le 6 novembre 2026

- [Conditions API Telegram](https://core.telegram.org/api/terms) : consentement,
  sécurité, identifiant API propre et transparence. La conformité d’un client
  spécialisé reste à évaluer, notamment l’exigence de fonctionnalités de base au
  paragraphe 1.3. Cet examen n’est pas une certification Telegram.
- [Licence des contenus Telegram](https://telegram.org/tos/content-licensing) :
  l’accès technique ne donne pas une autorisation générale de réutilisation.
- [Page publique @DeezLoad2Bot](https://t.me/DeezLoad2Bot) : canal et contact DMCA,
  sans conditions ni licence des fichiers sur la page consultée. Les droits du
  catalogue restent **non vérifiés**. L’utilisateur a demandé de poursuivre
  l’implémentation après ce constat. L’application présente cette limite avant
  connexion ; elle ne certifie aucun droit sur les fichiers reçus.
- [Connexion QR Telegram](https://core.telegram.org/api/qr-login) : validation dans
  une application déjà connectée, expiration et renouvellement du jeton.

N’utiliser que des fichiers dont le téléchargement est autorisé. Un abonnement
musical ou un lien vers une piste ne prouve pas cette autorisation.

## Dépendances et licences

Le pont utilise Telethon plutôt que TDLib. Aucune nouvelle dépendance Rust.
Versions fixées dans `integrations/telegram/requirements.txt` ; licences relevées
sur les distributions installées et copiées dans `integrations/telegram/licenses` :

| Dépendance | Version | Licence |
| --- | --- | --- |
| Telethon | 1.45.0 | MIT |
| qrcode | 8.2 | BSD-3-Clause, provenance MIT mentionnée dans sa licence |
| pyaes | 1.6.1 | MIT |
| rsa | 4.9.1 | Apache-2.0 |
| pyasn1 | 0.6.1 | BSD-2-Clause |

Telethon requiert pyaes et rsa ; rsa requiert pyasn1. qrcode n’a pas de dépendance
obligatoire supplémentaire sous Linux. Aucun extra cryptg, Pillow ou PNG utilisé.
Conserver les notices lors de la redistribution, y compris celles du runtime
Python (PSF) si celui-ci est fourni. Le fichier de versions n’est pas un verrou
avec empreintes ; réexaminer licences et sécurité lors de toute mise à jour.
Sources : [Telethon](https://github.com/LonamiWebs/Telethon/blob/v1/LICENSE),
[qrcode](https://github.com/lincolnloop/python-qrcode/blob/main/LICENSE),
[pyaes](https://github.com/ricmoo/pyaes),
[rsa](https://github.com/sybrenstuvel/python-rsa),
[pyasn1](https://pypi.org/project/pyasn1/0.6.1/).

## Limites techniques

- Une opération à la fois, aucun traitement d’historique ni téléchargement dans
  une autre conversation. Une nouvelle recherche remplace la fenêtre précédente.
- Les menus textuels, boutons de rappel et recherches inline sont affichés ; chaque choix
  exige un clic explicite. Les modifications de messages mettent à jour le menu.
  Les recherches inline affichent au maximum les 50 premiers résultats, sans pagination.
  Sélectionner un résultat le transmet explicitement au bot.
  Les boutons de lien, paiement, connexion, téléphone et localisation sont exclus.
  Le protocole privé du bot reste non garanti ; le parcours complet de recherche
  et téléchargement réel reste à valider. Un menu expire avec la réception.
- Audio déclaré par Telegram, maximum 512 Mio, signatures FLAC/MP3/Ogg/WAV/M4A.
  La vérification de signature n’est pas un décodage complet ni un antivirus.
- Noms générés localement : aucun chemin proposé par le bot n’est utilisé.
  Création exclusive, destination ouverte par descripteur, publication sans
  écrasement et suppression des fichiers partiels après échec ou annulation.
  Une interruption brutale du système peut laisser un fichier caché `.part`.
- Les limites Telegram sont affichées sans nouvel essai automatique. La réception
  expire après cinq minutes ; un transfert commencé a un délai maximal de dix
  minutes. Maximum 100 réponses en attente, dix boutons visibles à la fois.
- L’affichage GPUI du panneau et le QR n’ont pas été testés visuellement ici.
  Si le pont s’arrête à cause d’une installation manquante, corriger les prérequis
  puis relancer Onde.

## Validation

```sh
cargo check --offline
cargo test --offline
~/.local/share/onde/telegram-venv/bin/python -m unittest discover -s integrations/telegram -v
```

Compilation vérifiée ; 88 tests Rust réussis, 7 tests d’intégration préexistants
ignorés. Les tests Rust ont nécessité une exécution hors bac à sable pour leurs
serveurs HTTP locaux et dossiers de travail.

Neuf tests Python réussis : rejet des réponses d’une autre conversation ou
requête, validation des recherches vides ou trop longues, rejet d’une signature non audio, parcours avec transport simulé couvrant
filtrage des boutons sensibles ou externes, connexion sans téléchargement, réception corrélée, confirmation d’un fichier
ambigu, permissions, annulation, déconnexion et suppression locale.

Aucun compte réel connecté, aucun message réel envoyé et aucun téléchargement
auprès du bot effectué. Les essais réels restent à effectuer volontairement avec
un compte de test et des fichiers autorisés ; le fonctionnement de bout en bout
avec @DeezLoad2Bot n’est pas déclaré validé.

## Recherche et détails des résultats

Après un envoi explicite, Onde sélectionne une fois « Global search » dans le
menu reçu. La sélection d’un résultat reste manuelle. Les lignes affichent le
titre, la description du bot et l’artiste si présent dans les attributs audio.
Telegram ne garantit pas de champ album dans les résultats inline ; Onde affiche
les informations disponibles sans déduire un album à partir du titre.

## Réception automatique après sélection

Un résultat inline choisi est posté par le compte de l’utilisateur « via » le bot,
qui y ajoute ensuite l’audio en modifiant le message, de quelques secondes à plus
d’une minute. Onde surveille donc le chat jusqu’à trois minutes après le clic.
Envoyer un lien active la réception des nouveaux audios du bot pendant cinq minutes.
Les anciens messages et autres conversations restent exclus. Les transferts utilisent le dossier musical configuré au moment
de l’envoi, puis rafraîchissent la bibliothèque. Arrêter la réception termine
cette autorisation. En l’absence de référence fournie par le bot, cette association
est temporelle : éviter des demandes concurrentes au même bot dans un autre client.
Cette règle remplace la confirmation des fichiers ambigus après sélection.

Voir [Vérification audio et noms de fichiers](audio-quality.md) pour les mesures spectrales, le test FLAC et les changements de réception.
