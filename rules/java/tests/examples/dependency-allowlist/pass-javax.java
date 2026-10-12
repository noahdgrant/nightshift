import javax.annotation.processing.Processor;
import javax.swing.JFrame;
import javax.xml.XMLConstants;
import javax.xml.parsers.*;

class Ui {
    JFrame frame;
    Processor processor;
    DocumentBuilder builder;
    String ns = XMLConstants.XML_NS_URI;
}
