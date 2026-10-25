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
- Crea un inventario de entradas y una lista preliminar de nombres de archivos que podrían relacionarse con pesca, objetos, inventario, mercado o recursos.
- Publica informes durante siete días. El APK original no se incluye en el artefacto.

## Límites importantes

Este es un primer análisis estático; no ejecuta el juego ni configura un proveedor de IA. Los nombres de archivo que coinciden con palabras clave son solo pistas, no evidencia de la lógica del juego. Albion Online puede implementar buena parte de su economía y mecánicas en el servidor, y el APK puede contener código nativo u ofuscado que requiera herramientas adicionales. No se garantiza que el análisis revele la fórmula exacta de rendimiento de pesca.

La primera ejecución real de GitHub Actions es necesaria para validar que el entorno del runner, la versión instalada de REA y el APK concreto funcionen juntos. No se debe considerar probado hasta revisar los logs y el artefacto resultante.
