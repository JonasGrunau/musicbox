// Lists the callers of functions: for each target address, every reference to it with the
// function containing the reference (address, name).
// Args: <outFile> <0xaddress>...
// @category musicbox
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Reference;
import java.io.PrintWriter;

public class MbCallers extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        try (PrintWriter w = new PrintWriter(args[0], "UTF-8")) {
            for (int i = 1; i < args.length; i++) {
                Address target = toAddr(args[i]);
                Function tf = getFunctionAt(target);
                w.println(args[i] + "\t" + (tf == null ? "?" : tf.getName(true)));
                for (Reference r : getReferencesTo(target)) {
                    Function f = getFunctionContaining(r.getFromAddress());
                    w.println("  from 0x" + r.getFromAddress() + "\t" + r.getReferenceType() + "\t"
                            + (f == null ? "?" : "0x" + f.getEntryPoint() + " " + f.getName(true)));
                }
            }
        }
        println("WROTE " + args[0]);
    }
}
