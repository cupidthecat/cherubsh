for w in apple banana cherry date; do
  case $w in
    a*) echo "A: $w" ;;
    b*|c*) echo "BC: $w" ;;
    *) echo "other: $w" ;;
  esac
done
