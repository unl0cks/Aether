package flash.system {
    [Ruffle(Abstract)]
    public final class System {
        import __ruffle__.stub_method;
        import __ruffle__.stub_getter;

        public static native function gc():void;

        public static function pauseForGCIfCollectionImminent(imminence:Number = 0.75):void {
            stub_method("flash.system.System", "pauseForGCIfCollectionImminent");
        }

        public static native function setClipboard(string:String):void;

        public static function disposeXML(node:XML):void {
            stub_method("flash.system.System", "disposeXML");
        }

        public static function get freeMemory():Number {
            stub_getter("flash.system.System", "freeMemory");
            return 1024*1024*10; // 10MB
        }

        public static function get privateMemory():Number {
            stub_getter("flash.system.System", "privateMemory");
            return 1024*1024*100; // 100MB
        }

        public static native function get totalMemoryNumber():Number;

        public static function get totalMemory():uint {
            // `as uint` is a modulo conversion, not a clamp. Now that this reports a real figure it
            // can exceed 4 GiB, and wrapping would hand content a small number at the exact moment
            // memory is at its worst -- AQW tests `totalMemory > 200 * 1024 * 1024` to decide how
            // hard to collect, so a wrap there reads as "plenty free". `totalMemoryNumber` exists
            // precisely because this getter cannot represent large values, and keeps the true one.
            var bytes:Number = totalMemoryNumber;
            return bytes >= 4294967295 ? 4294967295 : bytes as uint;
        }
    }
}
