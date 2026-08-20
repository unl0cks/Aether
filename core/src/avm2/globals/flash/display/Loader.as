package flash.display {
    import __ruffle__.stub_method;

    import flash.display.LoaderInfo;
    import flash.display.DisplayObject;
    import flash.errors.IllegalOperationError;
    import flash.system.LoaderContext;
    import flash.system.System;
    import flash.utils.ByteArray;
    import flash.net.URLRequest;
    import flash.events.UncaughtErrorEvents;

    [Ruffle(InstanceAllocator)]
    public class Loader extends DisplayObjectContainer {

        [Ruffle(NativeAccessible)]
        private var _contentLoaderInfo:LoaderInfo;

        public function get contentLoaderInfo():LoaderInfo {
            return this._contentLoaderInfo;
        }

        public function get content():DisplayObject {
            return this._contentLoaderInfo.content;
        }

        public native function load(request:URLRequest, context:LoaderContext = null):void;

        public native function loadBytes(data:ByteArray, context:LoaderContext = null):void;

        public native function unload():void;

        [API("662")]
        public function unloadAndStop(gc:Boolean = true):void {
            // Still a stub for the "stop" half: unload halts the content's timelines, but not the
            // sounds or timers it started. The `gc` argument is honoured, which it previously was
            // not -- AQW calls `petLoader.unloadAndStop(true)` on every pet change, and asking for
            // a collection right after making a loader's content unreachable is the entire point
            // of passing it.
            stub_method("flash.display.Loader", "unloadAndStop");
            this.unload();
            if (gc) {
                System.gc();
            }
        }

        public function close():void {
            stub_method("flash.display.Loader", "close");
        }

        override public function addChild(child:DisplayObject):DisplayObject {
            throw new IllegalOperationError("Error #2069: The Loader class does not implement this method.", 2069);
        }

        override public function addChildAt(child:DisplayObject, index:int):DisplayObject {
            throw new IllegalOperationError("Error #2069: The Loader class does not implement this method.", 2069);
        }

        override public function removeChild(child:DisplayObject):DisplayObject {
            throw new IllegalOperationError("Error #2069: The Loader class does not implement this method.", 2069);
        }

        override public function removeChildAt(index:int):DisplayObject {
            throw new IllegalOperationError("Error #2069: The Loader class does not implement this method.", 2069);
        }

        override public function setChildIndex(child:DisplayObject, index:int):void {
            throw new IllegalOperationError("Error #2069: The Loader class does not implement this method.", 2069);
        }

        [API("667")]
        public function get uncaughtErrorEvents():UncaughtErrorEvents {
            return this.contentLoaderInfo.uncaughtErrorEvents;
        }
    }
}
