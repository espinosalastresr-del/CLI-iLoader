# Análisis integral del APK de Albion Online

Workflow: **Actions → REA Albion Android APK Analysis → Run workflow**.

URL predeterminada:
`https://live.albiononline.com/clients/20261015105018/Albion-Online.apk`

El workflow valida que la URL use HTTPS, pertenezca a `live.albiononline.com` y termine en `.apk`. El APK original no se publica en el artefacto; los informes se conservan durante 14 días.

## Etapas incluidas

1. Descarga con reintentos, tipo de archivo, tamaño, SHA-256 e integridad ZIP/CRC.
2. Inspección del paquete Android con REA.
3. Decodificación del manifiesto y recursos con Apktool.
4. Inventario del APK, resúmenes por extensión/directorio, archivos grandes, DEX, bibliotecas nativas, datos de Unity, archivos de configuración y cadenas estáticas candidatas.
5. Inventario de todos los recursos decodificados de `assets/GameData`: tamaño, SHA-256, entropía, prefijos binarios repetidos, cadenas imprimibles, grupos de contenido idéntico y variantes regionales.
6. Extracción de metadatos y bibliotecas IL2CPP, con intentos best-effort de reconstrucción.
7. Extracción de `global-metadata.dat`, `libil2cpp.so` y `libunity.so` para ARM64 y, si existe, IL2CPP ARMv7.
8. Intento de reconstrucción de ensamblados gestionados con Cpp2IL.
9. Intento independiente con Il2CppDumper para generar `dump.cs`, DLL ficticias, `il2cpp.h` y scripts compatibles con herramientas como Ghidra cuando la versión sea compatible.
10. Inspección ELF de bibliotecas nativas mediante `readelf`, extracción de símbolos, dependencias y cadenas ASCII/UTF-16LE.
11. Búsqueda de nombres relevantes en clases Android relacionados con red, autenticación, cuenta, inventario, objetos, mercado, plata, recursos, gremios, fabricación, comercio y pesca.
12. Registro de códigos de salida, logs, índices de salidas y hashes SHA-256 de informes.

## Dónde mirar primero

- `analysis-coverage.md/json`: resultado por herramienta. `0` indica que el comando terminó sin error; un código distinto o `not-run` requiere revisar los logs.
- `game-data-summary.md/json`: resumen de los blobs GameData y sus agrupaciones.
- `game-data-inventory.json`: inventario por archivo con tamaño, hash, entropía, prefijo y categoría aproximada.
- `game-data-regional-variants.json` y `game-data-duplicate-files.json`: comparación de variantes regionales y archivos idénticos.
- `game-data-printable-strings.json`: cadenas legibles halladas dentro de los datos, cuando existan.
- `cpp2il/`: salida que Cpp2IL haya conseguido generar.
- `il2cppdumper/`: salida de Il2CppDumper.
- `native/`: encabezados ELF, dependencias dinámicas, símbolos y cadenas de las bibliotecas.
- `il2cpp-metadata-strings.txt` y `il2cpp-keyword-hits.txt`: cadenas extraídas de los metadatos.
- `static-string-indicators.json/txt`: URLs, dominios, rutas tipo API y otras cadenas candidatas.
- `apk-files.json/txt` y `unity-assets-and-metadata.json`: inventario para localizar recursos y formatos personalizados.

## Limitaciones

Este pipeline hace análisis estático amplio; no reconstruye perfectamente el código fuente original. En la ejecución analizada, el APK tenía 4.023 entradas, pesaba 211.172.778 bytes y su SHA-256 era `b530e9815728d753a0a1c34a13e38dcc790bf3e3cd66a0c5f017d8e92439ec40`. Se extrajeron 215.850 cadenas imprimibles únicas de bibliotecas/metadatos y 5.880 coincidencias candidatas por palabras clave; esas coincidencias necesitan revisión contextual.

La cabecera observada de `global-metadata.dat` comienza por `0x12724394`, no por el valor habitual `0xFAB11BAF` de IL2CPP. Muchos archivos `GameData/*.bin` comienzan con los mismos 8 bytes `adbaca946fdb30f7` y muestran entropía muy alta. Son señales compatibles con un contenedor personalizado, compresión o cifrado, pero por sí solas **no prueban** cuál de esas técnicas se usa. El nuevo inventario documenta las evidencias sin afirmar que los blobs estén descifrados.

En la primera ejecución verde, Cpp2IL salió con código 255 porque el workflow utilizaba la opción no reconocida `--output-to`; se corrigió a `--output-root`. Il2CppDumper salió con código 134 porque intentó usar `Console.ReadKey` con entrada redirigida; el workflow ahora lo ejecuta con pseudo-terminal y una entrada de tecla no interactiva para exponer el error real. Debe ejecutarse el workflow otra vez para verificar estos ajustes; no hay que interpretar el CI verde anterior como una reconstrucción exitosa.

Los datos como `items.bin` y `loot.bin` pueden requerir ingeniería inversa específica de formato. Las cadenas de red son pistas, no prueba de endpoints activos; el workflow no ejecuta el juego ni contacta esos endpoints. La lógica autoritativa del multijugador puede residir en servidores y no estar en el APK.

Cpp2IL e Il2CppDumper son pasos best-effort. Sus errores deben quedar documentados sin bloquear la publicación de otros informes. Por tanto, un job verde no garantiza que todas las herramientas hayan reconstruido datos: consulta el informe de cobertura y los artefactos.
