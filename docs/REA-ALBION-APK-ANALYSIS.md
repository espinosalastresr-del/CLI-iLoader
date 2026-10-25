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
5. Extracción de `global-metadata.dat`, `libil2cpp.so` y `libunity.so` para ARM64 y, si existe, IL2CPP ARMv7.
6. Intento de reconstrucción de ensamblados gestionados con Cpp2IL.
7. Intento independiente con Il2CppDumper para generar `dump.cs`, DLL ficticias, `il2cpp.h` y scripts compatibles con herramientas como Ghidra cuando la versión sea compatible.
8. Inspección ELF de bibliotecas nativas mediante `readelf`, extracción de símbolos, dependencias y cadenas ASCII/UTF-16LE.
9. Búsqueda de nombres relevantes en clases Android relacionados con red, autenticación, cuenta, inventario, objetos, mercado, plata, recursos, gremios, fabricación, comercio y pesca.
10. Registro de códigos de salida, logs, índices de salidas y hashes SHA-256 de informes.

## Dónde mirar primero

- `analysis-coverage.md/json`: resultado por herramienta. `0` indica que el comando terminó sin error; un código distinto o `not-run` requiere revisar los logs.
- `cpp2il/`: salida que Cpp2IL haya conseguido generar.
- `il2cppdumper/`: salida de Il2CppDumper.
- `native/`: encabezados ELF, dependencias dinámicas, símbolos y cadenas de las bibliotecas.
- `il2cpp-metadata-strings.txt` y `il2cpp-keyword-hits.txt`: cadenas extraídas de los metadatos.
- `static-string-indicators.json/txt`: URLs, dominios, rutas tipo API y otras cadenas candidatas.
- `apk-files.json/txt` y `unity-assets-and-metadata.json`: inventario para localizar recursos y formatos personalizados.

## Limitaciones

Este pipeline hace análisis estático amplio; no reconstruye perfectamente el código fuente original. Albion Online usa Unity IL2CPP: los métodos pueden recuperarse solo parcialmente y las herramientas pueden no admitir la versión concreta de metadatos o protecciones. Los datos como `items.bin` y `loot.bin` pueden requerir ingeniería inversa específica de formato. Las cadenas de red son pistas, no prueba de endpoints activos; el workflow no ejecuta el juego ni contacta esos endpoints. La lógica autoritativa del multijugador puede residir en servidores y no estar en el APK.

Cpp2IL e Il2CppDumper son pasos best-effort. Sus errores deben quedar documentados sin bloquear la publicación de otros informes. Por tanto, un job verde no garantiza que todas las herramientas hayan reconstruido datos: consulta el informe de cobertura y los artefactos.
