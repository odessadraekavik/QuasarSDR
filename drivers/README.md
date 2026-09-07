# Runtime RTL-SDR Windows x64

Ce dossier contient localement les bibliothèques déjà présentes dans
`C:\Users\Odessa\Downloads\Release\x64` : `rtlsdr.dll`, `msvcr100.dll`,
`pthreadVC2.dll`. Aucun installateur et aucun changement de driver Windows
n'ont été exécutés par QuasarSDR.

Les DLL ne sont pas suivies dans Git. Sur un autre poste, les prendre dans une
distribution RTL-SDR Blog V4 compatible et les placer ici ou dans `drivers/`
à côté du binaire. Conserver les licences et les informations de provenance de
la distribution utilisée.

- [Guide V4 officiel](https://www.rtl-sdr.com/v4/)
- [Sources et distributions du driver](https://github.com/rtlsdrblog/rtl-sdr-blog)

Le chargement utilise un chemin absolu, la recherche des dépendances dans le
dossier de la DLL et les emplacements système Windows. Aucune DLL ne sera
téléchargée ou installée automatiquement à l'ouverture de l'application.
