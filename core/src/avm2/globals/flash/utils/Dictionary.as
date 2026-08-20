package flash.utils {
    [Ruffle(InstanceAllocator)]
    public dynamic class Dictionary {
        prototype.toJSON = function(r:String):* {
            return "Dictionary";
        };
        prototype.setPropertyIsEnumerable("toJSON", false);

        public function Dictionary(weakKeys:Boolean = false) {
            if (weakKeys) {
                this.initWeakKeys();
            }
        }

        // Called only from the constructor, and only once. Splitting it out is what lets the
        // argument reach the object at all: the instance allocator runs before any argument
        // exists, so the flag cannot be set there.
        private native function initWeakKeys():void;
    }
}
