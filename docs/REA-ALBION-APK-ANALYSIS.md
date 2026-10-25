# Análisis del APK de Albion Online con REA

Workflow: **Actions → REA Albion Android APK Analysis → Run workflow**.

URL predeterminada:
`https://live.albiononline.com/clients/20261015105018/Albion-Online.apk`

El workflow valida que la URL use HTTPS, pertenezca a `live.albiononline.com` y termine en `.apk`. Puedes dejar el valor predeterminado al iniciar la ejecución. No requiere crear un secreto para esta URL pública.

## Qué hace

- Descarga el APK con reintentos y registra tamaño y SHA-256.
- Verifica la estructura ZIP, los CRC de los miembros y la presencia de `AndroidManifest.xml`.
- Instala Java 21, una versión fijada de REA y el motor headless JADX 0.7.1; valida el SHA-256 del JAR del motor.
- Ejecuta `rea inspect-android-package` y guarda la salida JSON, stderr y código de salida.
- Detecta el runtime Unity IL2CPP y extrae `assets/bin/Data/Managed/Metadata/global-metadata.dat` con un límite de tamaño.
- Ejecuta `rea inspect-il2cpp-metadata` y genera una lista preliminar de coincidencias con términos de pesca, recolección, inventario, mercado y recursos.
- Ejecuta un análisis estático general del APK: inventario completo, resúmenes por extensión y directorio, archivos mayores, bibliotecas nativas, DEX, recursos Unity/metadatos y archivos de configuración/texto. Escanea cadenas imprimibles en código, bibliotecas, metadatos y archivos seleccionados para extraer URLs, dominios, posibles direcciones IPv4, rutas tipo API y cadenas relacionadas con red, autenticación, almacenamiento, telemetría y sistemas del juego. El análisis tiene límites explícitos de tamaño y no contacta los endpoints encontrados.
- Conserva el inventario completo y los indicadores como JSON y texto; genera hashes de los informes para facilitar la reproducibilidad.
- Publica informes durante siete días. El APK original no se incluye en el artefacto.

## Límites importantes

Este es un análisis estático amplio; no ejecuta el juego ni contacta los endpoints encontrados. Las URLs, dominios, rutas API y cadenas detectadas son indicadores candidatos y requieren validación manual: pueden estar incompletos, ser obsoletos o aparecer en bibliotecas de terceros. El escaneo de cadenas tiene límites de tamaño, no descomprime ni interpreta todos los formatos de Unity, y no equivale a descompilar la lógica nativa. Albion Online usa Unity IL2CPP: los metadatos pueden revelar nombres de tipos, métodos y campos, pero no garantizan el cuerpo de los métodos. Para estudiar funciones con más profundidad harán falta herramientas adicionales compatibles con IL2CPP y, cuando proceda, análisis dinámico autorizado. La lógica autoritativa de economía, combate y estado multijugador puede residir en los servidores.

El primer análisis de Android/JADX completó correctamente. El APK contiene `libil2cpp.so` y `global-metadata.dat`, por lo que el workflow también intenta inspeccionar metadatos IL2CPP. Los pasos nuevos de análisis integral y metadatos deben considerarse pendientes de validación hasta que una nueva ejecución de GitHub Actions termine y revisemos los logs y artefactos.
