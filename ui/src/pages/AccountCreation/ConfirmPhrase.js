import { Button } from 'semantic-ui-react';
import HyperLinkButton from '../../components/HyperLinkButton';
import "../../App.css";
import { useOutletContext } from 'react-router-dom';

const ConfirmPhrase = () => {

  const {
    goBack,
    goForward,
    onClickSelectWordButton,
    selectedRecoveryPhrase,
    shuffledRecoveryPhrase,
    isValidSelectedPhrase
  } = useOutletContext();

  return (<>
    <div className='tallHeaderContainer'>
      <h1 className='mainheadline'>Confirm Your Secret Recovery Phrase</h1>
      <p className='subtext'>
        Please select the appropriate phrase in the correct order.
      </p>
    </div>

    <div className='wordListContainer'>
      {selectedRecoveryPhrase.map(function (item, index) {
        let word = item.split("_")[0];
        return <div className='button ui buttonlist' key={index}>{word}</div>
      })}
    </div>

    <div className='buttonListContainer'>
      {shuffledRecoveryPhrase.map(function (item) {
        let word = item.split("_")[0];
        return (
          <Button
            onClick={(e) => onClickSelectWordButton(e, item)}
            className="button ui buttonlist"
            key={item}>
            {word}
          </Button>
        )
      })}
    </div>
    <Button disabled={!isValidSelectedPhrase} className="button ui first wide" onClick={goForward}>
      Confirm
    </Button>
    <HyperLinkButton
      text={"Go Back"}
      onclick={goBack}
    />

  </>)
}

export default ConfirmPhrase;