Conçois l'interface complète d'Onde, un lecteur de musique de bureau, sous forme de maquette HTML interactive en un seul fichier. Je veux une direction artistique forte et mémorable, pas un clone de Spotify ni un tableau de bord sombre générique.

## Le produit

Onde réunit toute la musique d'une personne au même endroit : ses fichiers locaux, son serveur personnel (Navidrome), puis d'autres sources. C'est une app native rapide, pensée pour le plaisir d'écouter : la musique et sa pochette occupent l'espace, et l'image réagit vraiment au son. Le nom évoque les ondes sonores et l'eau.

Tu peux changer librement le logo, la typo et les couleurs. L'actuel fond 0x12141a et l'accent turquoise 0x3fd0c0 ne sont qu'un point de départ.

## Écrans à dessiner

1. **Accueil** : reprises d'écoute, ajouts récents, playlists, source par source ou tout mélangé.
2. **Bibliothèque** : morceaux, albums, artistes, avec tri et recherche. Elle doit rester agréable avec 20 000 morceaux.
3. **Album, artiste et playlist** : pages avec pochette en grand, et couleurs tirées de la pochette.
4. **Lecture plein écran** : l'écran signature. Pochette, visuels réactifs, paroles et commandes.
5. **Paroles** : ligne par ligne, avec remplissage mot à mot, lisibles par-dessus les visuels.
6. **File d'attente** : panneau latéral, avec réorganisation par glisser-déposer.
7. **Recherche et téléchargement** : chercher un morceau, voir de quelle source il vient, l'ajouter à la bibliothèque du serveur, suivre la progression et les erreurs.
8. **Sources** : connecter et voir l'état de chaque source (dossier local, Navidrome, SoundCloud, YouTube, Spotify).
9. **Réglages** : égaliseur, vitesse et hauteur, choix des visuels, raccourcis.
10. **Mini-lecteur** : petite fenêtre toujours visible.

La barre de lecture doit montrer le titre, l'artiste, la pochette, précédent/lecture/suivant, la progression, le volume, et les accès aux paroles, à la file et aux visuels.

Pense aussi aux états : bibliothèque vide, chargement, source hors ligne, morceau sans pochette, pas de paroles, et fenêtre étroite (moins de 760 px).

## Visuels réactifs : le cœur du projet

Les visuels actuels (des barres de spectre et un halo léger) sont jugés trop basiques. Propose **au moins 4 visuels vraiment différents**, sélectionnables dans la maquette, dynamiques et spectaculaires, sans gêner la lecture des paroles. Évite le simple égaliseur à barres.

Données disponibles 30 fois par seconde :
- 48 bandes de fréquences, de 40 Hz à 16 kHz, de 0 à 1, à niveau déjà normalisé ;
- l'enveloppe des basses, de 0 à 1 ;
- la forme d'onde récente, en 2048 échantillons ;
- les couleurs dominantes de la pochette ;
- on peut en calculer d'autres si tu les décris : détection des coups de batterie, énergie des médiums et des aigus, tempo, moments forts.

Simule ces données en JavaScript avec un faux morceau (kick, basse, mélodie, couplet et refrain), sans fichier audio. La maquette doit donc bouger comme avec de la vraie musique, avec un bouton lecture/pause. En pause, les visuels retombent doucement jusqu'à l'immobilité.

## Contraintes de rendu : à respecter

L'app sera recodée en Rust avec GPUI, pas en HTML. N'utilise dans la maquette que ce que GPUI peut reproduire :

- **Disponible** : rectangles à coins arrondis, bordures, ombres floues colorées, dégradés linéaires à deux couleurs, formes vectorielles remplies (tracés), images, texte avec couleur par portion, opacité, listes virtualisées, animations recalculées à chaque image.
- **À considérer comme indisponible** : flou d'arrière-plan et verre dépoli (`backdrop-filter`), `filter` CSS, shaders et WebGL, modes de fusion (`mix-blend-mode`), masques, `clip-path`, dégradés radiaux ou coniques, vidéo. Un halo se fait avec une ombre floue. Pour les visuels, dessine sur un canvas 2D uniquement avec des rectangles arrondis, des tracés remplis et des ombres.

Vise au plus environ 500 formes par image : l'app doit rester légère. Pour rester crédible, ne fais pas dépendre un visuel d'une simulation de milliers de particules.

## Interaction

Tout doit être dense mais calme, au clavier comme à la souris. Raccourcis déjà en place : espace (lecture), flèches gauche/droite (±5 s), Ctrl+flèches (morceau précédent/suivant), haut/bas (volume), `l` (paroles), `v` (visuels), `n` (Navidrome). Décris les transitions entre écrans et l'état de survol, de focus et d'appui de chaque composant.

## Ce que je veux recevoir

1. La maquette interactive, navigable entre tous les écrans, avec des données fictives : morceaux et artistes inventés, pochettes générées en dégradés ou formes abstraites, sans marques ni artistes réels.
2. Une feuille de style de design à la fin du fichier ou dans un panneau dédié : couleurs, typographies et tailles, espacements, rayons, ombres, durées d'animation, le tout avec des valeurs exactes.
3. Pour chaque visuel, une description courte de sa recette : quelles données pilotent quelles formes, combien de formes par image, et comment il s'efface sous les paroles.

Commence par me donner en quelques lignes ton parti pris artistique, puis construis directement la maquette.
