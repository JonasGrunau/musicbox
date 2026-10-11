// Lists functions whose qualified name contains a substring, with entry address and size.
// Args: <outFile> <substring>...
// @category musicbox
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.Function;
import java.io.PrintWriter;

public class MbListFunctions extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        try (PrintWriter w = new PrintWriter(args[0], "UTF-8")) {
            for (Function f : currentProgram.getFunctionManager().getFunctions(true)) {
                String name = f.getName(true);
                for (int i = 1; i < args.length; i++) {
                    if (name.contains(args[i])) {
                        w.println("0x" + f.getEntryPoint() + "\t" + f.getBody().getNumAddresses() + "\t" + name);
                        break;
                    }
                }
            }
        }
        println("WROTE " + args[0]);
    }
}
